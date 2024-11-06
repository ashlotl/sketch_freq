use core::f32;
use std::{
    sync::{
        atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicUsize, Ordering},
        mpsc::{self, Receiver},
        Arc, Mutex,
    },
    thread,
    time::Instant,
};

use lv2::prelude::*;
use winit::platform::x11::EventLoopBuilderExtX11;
use wmidi::{MidiMessage, Note};

use crate::{
    frontend::Frontend,
    shared_data::{ChannelSelection, Data, DataWriter},
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

    midi_events: InputPort<AtomPort>,

    output: OutputPort<Audio>,
}

#[uri("https://example.com/changethislater")]
pub struct SynthWrapper {
    inner: Arc<Synth>,
}

pub struct Synth {
    data: Arc<DataWriter>,
    utilization: Arc<AtomicU32>,
    debug: Arc<AtomicI32>,
    last_instant: Mutex<Instant>,
    sample_rate: u32,
    studied_samples: AtomicU32,

    /// There are 128 possible midi notes and for each note we need to store a cache of the computed values for each frame after a note trigger.
    /// We store two versions of the cache described above and write into whichever one is not currently being read out of to produce sound.
    /// When a cache recompute finishes, we switch the active cache again. See `active_cache`.
    sum_cache: [Mutex<[Vec<f32>; 128]>; 2],

    /// Index for which cache is being used to produce audio. Valid values are 0 or 1.
    active_cache: AtomicUsize,

    // there are 128 possible midi notes, each of which can be retriggered at a speed presumably less than the heap can deal with
    active_midi_notes: Mutex<[Vec<(SampleTime, MidiMessage<'static>)>; 128]>,

    midi_sequence_urid: URID<Sequence>,
    midi_event_urid: URID<MidiEvent>,
    midi_beat_urid: URID<Beat>,
}

impl Plugin for SynthWrapper {
    type Ports = Ports;

    type InitFeatures = Features<'static>;
    type AudioFeatures = ();

    fn new(plugin_info: &PluginInfo, features: &mut Features) -> Option<Self> {
        let (invalidate_cache_send, invalidate_cache_read) = {
            let (tx, rx) = mpsc::channel();
            (Arc::new(tx), Arc::new(Mutex::new(rx)))
        };
        let reinvalidate_cache = Arc::new(AtomicBool::new(false));
        let reinvalidate_cache_clone = reinvalidate_cache.clone();
        let data = Arc::new(DataWriter::new(
            invalidate_cache_send,
            reinvalidate_cache,
            Data::new(),
        ));

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
                "sketch freq",
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

        let plot_duration = data.read_with(|data| data.plot_duration);

        let synth = Arc::new(Synth {
            data,
            sample_rate: plugin_info.sample_rate().round() as u32,
            studied_samples: AtomicU32::new(0),
            utilization,
            debug,
            last_instant: Mutex::new(Instant::now()),
            sum_cache: std::array::from_fn(|_i| {
                Mutex::new(std::array::from_fn(|_j| {
                    vec![0f32; (plugin_info.sample_rate().round() as f32 * plot_duration) as usize]
                }))
            }),
            active_cache: AtomicUsize::new(0),
            active_midi_notes: Mutex::new([const { vec![] }; 128]),
            midi_sequence_urid: features.urid_map.map_type().unwrap(),
            midi_beat_urid: features.urid_map.map_type().unwrap(),
            midi_event_urid: features.urid_map.map_type().unwrap(),
        });

        let synth_clone = synth.clone();
        std::thread::spawn(move || {
            compute_cache(
                synth_clone,
                &*invalidate_cache_read.lock().unwrap(),
                reinvalidate_cache_clone,
            );
        });

        Some(Self { inner: synth })
    }

    fn run(&mut self, ports: &mut Ports, _features: &mut (), _: u32) {
        let inner = &self.inner;
        let unused = {
            let mut lock = inner.last_instant.lock().unwrap();
            let ret = lock.elapsed().as_millis();
            *lock = Instant::now();
            ret
        };

        let plot_duration = inner.data.read_with(|data| data.plot_duration);

        let mut active_midi_notes = inner.active_midi_notes.lock().unwrap();
        ports
            .midi_events
            .read(inner.midi_sequence_urid, inner.midi_beat_urid)
            .unwrap()
            .for_each(|(_time_stamp, atom)| {
                let event = atom.read(inner.midi_event_urid, ()).unwrap();

                let wmidi_msg = MidiMessage::try_from(event).unwrap();

                //TODO: check channel
                let _channel = wmidi_msg.channel();

                if let wmidi::MidiMessage::NoteOn(_, note, _vel) = wmidi_msg {
                    active_midi_notes[u8::from(note) as usize].push((
                        inner.studied_samples.load(Ordering::SeqCst),
                        wmidi_msg.clone(),
                    ));
                }
                if let wmidi::MidiMessage::NoteOff(_, note, _vel) = wmidi_msg {
                    active_midi_notes[u8::from(note) as usize].push((
                        inner.studied_samples.load(Ordering::SeqCst),
                        wmidi_msg.clone(),
                    ));
                }
            });

        let coef = 10f32.powf((*ports.gain).min(90f32).max(-90f32) * 0.05);

        // respond to notes and play sound
        let active_cache_i = inner.active_cache.load(Ordering::SeqCst);
        let active_cache = inner.sum_cache[active_cache_i].lock().unwrap();
        for out_frame in ports.output.iter_mut() {
            *out_frame = 0f32;
            for pitch_index in 0..active_midi_notes.len() {
                for active_note_index in (0..active_midi_notes[pitch_index].len()).rev() {
                    let since_start = inner.studied_samples.load(Ordering::SeqCst)
                        - active_midi_notes[pitch_index][active_note_index].0;
                    if since_start >= (inner.sample_rate as f32 * plot_duration) as u32 {
                        active_midi_notes[pitch_index].remove(active_note_index);
                    } else {
                        let msg = &active_midi_notes[pitch_index][active_note_index].1;
                        let intensity = if let wmidi::MidiMessage::NoteOn(_, _note, vel) = msg {
                            10f32.powf(u8::from(*vel) as f32 / 128f32)
                        } else if let wmidi::MidiMessage::NoteOff(_, _note, vel) = msg {
                            10f32.powf((128 - u8::from(*vel)) as f32 / 128f32)
                        } else {
                            1f32
                        };

                        *out_frame +=
                            active_cache[pitch_index][since_start as usize] * coef * intensity;
                    }
                }
            }
        }

        // update utilization statistic
        {
            let mut lock = inner.last_instant.lock().unwrap();
            let used = lock.elapsed().as_millis();
            inner.utilization.store(
                (used as f32 / unused as f32 * 100f32) as u32,
                Ordering::Relaxed,
            );
            *lock = Instant::now();
        }
    }
}

fn compute_cache(synth: Arc<Synth>, invalidate: &Receiver<()>, reinvalidate: Arc<AtomicBool>) {
    loop {
        //wait for signal from frontend to do a computation of the cache
        invalidate.recv().unwrap();

        synth.data.read_with(|data| {
            let active_cache_i = synth.active_cache.load(Ordering::SeqCst);
            let mut active_cache = synth.sum_cache[(active_cache_i + 1) % 2].lock().unwrap();
            'sample: for sample in 0..(synth.sample_rate as f32 * data.plot_duration) as u32 {
                for midi_note in 0..128 {
                    let mut sum = 0f32;
                    for frequency_index in 0..data.frequency_count {
                        let frequency = Note::from_u8_lossy(midi_note).to_freq_f32()
                            * 2f32.powf(
                                (frequency_index as i32 - data.frequency_count as i32 / 2) as f32
                                    / (12 * data.semitone_divisions) as f32,
                            )
                            * 440f32;

                        let amplitude = data.amplitude_plot
                            [frequency_index as usize * data.x_width_plot() + sample as usize]
                            as f32;

                        if amplitude > 1f32 {
                            sum += (sample as f32 / synth.sample_rate as f32
                                * frequency
                                * 2f32
                                * f32::consts::PI)
                                .sin()
                                * 10f32.powf(amplitude / 255f32 - 1f32);
                        }

                        if reinvalidate.fetch_not(Ordering::SeqCst) {
                            break 'sample;
                        }
                    }
                    active_cache[midi_note as usize][sample as usize] = sum;
                }
            }
        });
    }
}
