use core::f32;
use std::{
    f64,
    sync::{
        atomic::{AtomicI32, AtomicU32, Ordering},
        Arc, RwLock,
    },
    thread,
    time::Instant,
};

use lv2::prelude::*;
use winit::platform::x11::EventLoopBuilderExtX11;
use wmidi::{MidiMessage, Note};

use crate::{
    frontend::Frontend,
    shared_data::{ChannelSelection, Data},
};

/// time in samples/frames
type SampleTime = u32;

#[derive(FeatureCollection)]
pub struct Features<'a> {
    urid_map: LV2Map<'a>,
}

pub struct MidiPitchMap(String);

#[derive(PortCollection)]
pub struct Ports {
    gain: InputPort<Control>,
    center_frequency: InputPort<Control>,
    pitch_scale: InputPort<Control>,

    midi_events: InputPort<AtomPort>,

    output: OutputPort<Audio>,
}

#[uri("https://example.com/changethislater")]
pub struct Amp {
    data: Arc<RwLock<Data>>,
    utilization: Arc<AtomicU32>,
    debug: Arc<AtomicI32>,
    last_instant: Instant,
    sample_rate: u32,
    studied_samples: u32,
    // there are 128 possible midi notes, for each note we need to store a cache of the computed values for each frame
    sum_cache: [Vec<f32>; 128],
    cache_recompute_at: Option<u32>,

    // there are 128 possible midi notes, each of which can be retriggered at a speed presumably less than the heap can deal with
    active_midi_notes: [Vec<(SampleTime, MidiMessage<'static>)>; 128],

    midi_sequence_urid: URID<Sequence>,
    midi_event_urid: URID<MidiEvent>,
    midi_beat_urid: URID<Beat>,
}

impl Plugin for Amp {
    type Ports = Ports;

    type InitFeatures = Features<'static>;
    type AudioFeatures = ();

    fn new(plugin_info: &PluginInfo, features: &mut Features) -> Option<Self> {
        let data = Arc::new(RwLock::new(Data::new()));
        let data_clone = data.clone();
        let utilization = Arc::new(AtomicU32::new(0));
        let debug = Arc::new(AtomicI32::new(0));

        let utilization_clone = utilization.clone();
        let debug_clone = debug.clone();
        thread::spawn(move || {
            let options = eframe::NativeOptions {
                viewport: egui::ViewportBuilder::default().with_inner_size([400.0, 800.0]),
                event_loop_builder: Some(Box::new(|event_loop_builder| {
                    event_loop_builder.with_any_thread(true).with_x11();
                })),
                ..Default::default()
            };
            eframe::run_native(
                "Sketch-a-Spectrum",
                options,
                Box::new(|_cc| {
                    Ok(Box::new(Frontend {
                        data: data_clone,
                        plot_texture_handle: None,
                        plot_r_channel_binding: 0,
                        plot_g_channel_binding: 1,
                        plot_b_channel_binding: 2,
                        brush_radius_x: 1.95f32,
                        brush_radius_y: 1f32,
                        brush_color: ChannelSelection::Red,
                        eraser_radius_x: 0.9f32,
                        eraser_radius_y: 4f32,
                        eraser_color: ChannelSelection::Red,
                        utilization: utilization_clone,
                        debug: debug_clone,
                    }))
                }),
            )
            .unwrap();
        });

        let data_read = data.read().unwrap();
        let plot_duration = data_read.plot_duration;

        drop(data_read);

        Some(Self {
            data,
            sample_rate: plugin_info.sample_rate().round() as u32,
            studied_samples: 0,
            utilization,
            debug,
            last_instant: Instant::now(),
            sum_cache: std::array::from_fn(|_i| {
                vec![0f32; (plugin_info.sample_rate().round() as f32 * plot_duration) as usize]
            }),
            cache_recompute_at: None,
            active_midi_notes: [const { vec![] }; 128],
            midi_sequence_urid: features.urid_map.map_type().unwrap(),
            midi_beat_urid: features.urid_map.map_type().unwrap(),
            midi_event_urid: features.urid_map.map_type().unwrap(),
        })
    }

    fn run(&mut self, ports: &mut Ports, _features: &mut (), _: u32) {
        let mut data = self.data.write().unwrap();

        ports
            .midi_events
            .read(self.midi_sequence_urid, self.midi_beat_urid)
            .unwrap()
            .for_each(|(_time_stamp, atom)| {
                let event = atom.read(self.midi_event_urid, ()).unwrap();

                let wmidi_msg = MidiMessage::try_from(event).unwrap();

                //TODO: check channel

                if let wmidi::MidiMessage::NoteOn(_, note, vel) = wmidi_msg {
                    let volume = u8::from(vel) as f32 / 127.0;
                    self.active_midi_notes[u8::from(note) as usize]
                        .push((self.studied_samples, wmidi_msg.clone()));
                    println!("ON: {} at volume {}", note, volume);
                }
                if let wmidi::MidiMessage::NoteOff(_, note, _vel) = wmidi_msg {
                    println!("OFF: {}", note);
                }
            });

        let unused = self.last_instant.elapsed().as_millis();
        self.last_instant = Instant::now();

        let coef = 10f32.powf((*ports.gain).min(90f32).max(-90f32) * 0.05);

        //set point at which cache must be recomputed from
        if self.cache_recompute_at.is_none() || data.reinvalidate_cache {
            data.reinvalidate_cache = false;
            self.cache_recompute_at = Some(self.studied_samples);
        }

        if data.caches_invalidated {
            //do recompute here
            for out_frame in ports.output.iter_mut() {
                let time_elapsed = self.studied_samples as f64 / self.sample_rate as f64;
                let loop_index =
                    self.studied_samples % (self.sample_rate as f32 * data.plot_duration) as u32;
                let loop_time = (time_elapsed % data.plot_duration as f64) as f32;
                let loop_time_subdivision_index =
                    (loop_time * data.second_subdivisions as f32) as usize;

                for midi_note in 0..128 {
                    let mut sum = 0f32;
                    for frequency_index in 0..data.frequency_count {
                        let frequency = Note::from_u8_lossy(midi_note).to_freq_f32()
                            * 2f32.powf(
                                *ports.pitch_scale
                                    + (frequency_index as i32 - data.frequency_count as i32 / 2)
                                        as f32
                                        / (12 * data.semitone_divisions) as f32,
                            )
                            * *ports.center_frequency;

                        let amplitude =
                            data.amplitude_plot[frequency_index as usize * data.x_width_plot()
                                + loop_time_subdivision_index] as f32;

                        if amplitude > 1f32 {
                            sum += (loop_time * frequency * 2f32 * f32::consts::PI).sin()
                                * 10f32.powf(amplitude / 255f32 - 1f32);
                        }
                    }
                    self.sum_cache[midi_note as usize][loop_index as usize] = sum;
                }

                *out_frame = coef * sum as f32;
                self.studied_samples += 1;
            }

            //check if enough samples have been computed to consider the cache updated
            if self.studied_samples - self.cache_recompute_at.unwrap()
                >= (self.sample_rate as f32 * data.plot_duration) as u32
            {
                println!("done with recompute");
                data.caches_invalidated = false;
                self.cache_recompute_at = None;
            }
            //pretend that the cache was always fine by subtracting the timestep and proceeding as normal
            self.studied_samples -= ports.output.len() as u32;
        }

        // use precomputed values to write to output, barring considerations like velocity
        for out_frame in ports.output.iter_mut() {
            let loop_index =
                self.studied_samples % (self.sample_rate as f32 * data.plot_duration) as u32;
            *out_frame = self.sum_cache[loop_index as usize] * coef;
            self.studied_samples += 1;
        }

        // update utilization statistic
        let used = self.last_instant.elapsed().as_millis();
        self.utilization.store(
            (used as f32 / unused as f32 * 100f32) as u32,
            Ordering::Relaxed,
        );
        self.last_instant = Instant::now();

        //OTHERWISE MUST RECOMPUTE SUM CACHE

        let used = self.last_instant.elapsed().as_millis();
        self.utilization.store(
            (used as f32 / unused as f32 * 100f32) as u32,
            Ordering::Relaxed,
        );
        self.last_instant = Instant::now();
    }
}
