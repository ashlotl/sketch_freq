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
use wmidi::{MidiMessage, Note, U7};

use crate::{
    frontend::Frontend,
    shared_data::{ChannelSelection, Data, DataWriter},
};

/// time in samples/frames
type SampleTime = usize;

const CHUNK_SAMPLE_COUNT: usize = 1024;
const PLAUSIBLE_MIDI_FREQUENCIES: usize = 128;

pub fn chunk_indices_from_sample_range(sample_range: (usize, usize)) -> Vec<usize> {
    let start_chunk_i = sample_range.0 / CHUNK_SAMPLE_COUNT;
    let end_chunk_i = sample_range.1 / CHUNK_SAMPLE_COUNT;
    (start_chunk_i..=end_chunk_i).collect()
}

pub struct Chunk {
    data: Mutex<[[f32; PLAUSIBLE_MIDI_FREQUENCIES]; CHUNK_SAMPLE_COUNT]>,
    pub invalidate: AtomicBool,
}

#[derive(FeatureCollection)]
pub struct Features<'a> {
    urid_map: LV2Map<'a>,
}

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
    _debug: Arc<AtomicI32>,
    last_instant: Mutex<Instant>,
    pub sample_rate: usize,
    studied_samples: AtomicUsize,

    /// There are 128 possible midi notes and for each note we need to store a cache of the computed values for each frame after a note trigger.
    /// We store two versions of the cache described above and write into whichever one is not currently being read out of to produce sound.
    /// When a cache recompute finishes, we switch the active cache again. See `active_cache`.
    pub sum_cache: Vec<Chunk>,

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
        let sample_rate = plugin_info.sample_rate().round() as usize;

        let (recompute_cache_send, recompute_cache_read) = {
            let (tx, rx) = mpsc::channel();
            (Arc::new(tx), Arc::new(Mutex::new(rx)))
        };
        let interrupt = Arc::new(AtomicBool::new(false));
        let data = Arc::new(DataWriter::new(
            recompute_cache_send,
            interrupt.clone(),
            Data::new(sample_rate),
        ));

        let utilization = Arc::new(AtomicU32::new(0));
        let debug = Arc::new(AtomicI32::new(0));

        let plot_length = data.read_with(|data| data.plot_length);

        let sum_cache_len = plot_length / CHUNK_SAMPLE_COUNT + 1;
        let mut sum_cache = Vec::with_capacity(sum_cache_len);
        for _ in 0..sum_cache_len {
            sum_cache.push(Chunk {
                data: Mutex::new([[0f32; PLAUSIBLE_MIDI_FREQUENCIES]; CHUNK_SAMPLE_COUNT]),
                invalidate: AtomicBool::new(false),
            });
        }

        let synth = Arc::new(Synth {
            data: data.clone(),
            sample_rate,
            studied_samples: AtomicUsize::new(0),
            utilization: utilization.clone(),
            _debug: debug.clone(),
            last_instant: Mutex::new(Instant::now()),
            sum_cache,
            active_midi_notes: Mutex::new([const { vec![] }; 128]),
            midi_sequence_urid: features.urid_map.map_type().unwrap(),
            midi_beat_urid: features.urid_map.map_type().unwrap(),
            midi_event_urid: features.urid_map.map_type().unwrap(),
        });

        let synth_clone = synth.clone();
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
                    let plot_texture_x_resolution = 256;
                    let frequency_count = data.read_with(|data| data.frequency_count);
                    Ok(Box::new(Frontend {
                        data,
                        synth: synth_clone,
                        alias_protection: 0.05,
                        plot_texture_handle: None,
                        plot_texture_x_resolution: 256,
                        plot_texture_buffer_rgba: vec![
                            0;
                            plot_texture_x_resolution
                                * frequency_count
                                * 4
                        ],
                        plot_r_channel_binding: 0,
                        plot_g_channel_binding: 1,
                        plot_b_channel_binding: 2,

                        brush_radius_x: 4800f32,
                        brush_radius_y: 1f32,
                        brush_color: ChannelSelection::Red,
                        brush_opacity: 50f32,
                        eraser_radius_x: 0.9f32,
                        eraser_radius_y: 4f32,
                        eraser_color: ChannelSelection::Red,
                        utilization,
                        debug,
                    }))
                }),
            )
            .unwrap();
        });

        let synth_clone = synth.clone();
        std::thread::spawn(move || {
            compute_cache(
                synth_clone,
                &*recompute_cache_read.lock().unwrap(),
                interrupt,
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

        let plot_length = inner.data.read_with(|data| data.plot_length);

        let mut active_midi_notes = inner.active_midi_notes.lock().unwrap();
        ports
            .midi_events
            .read(inner.midi_sequence_urid, inner.midi_beat_urid)
            .unwrap()
            .for_each(|(_time_stamp, atom)| {
                let Some(event) = atom.read(inner.midi_event_urid, ()) else {
                    return;
                };

                let wmidi_msg = MidiMessage::try_from(event).unwrap();

                //TODO: check channel
                let _channel = wmidi_msg.channel();

                if let wmidi::MidiMessage::NoteOn(_, note, _vel) = wmidi_msg {
                    println!("note on: {note}");
                    active_midi_notes[u8::from(note) as usize].push((
                        inner.studied_samples.load(Ordering::SeqCst),
                        wmidi_msg.clone(),
                    ));
                }
                if let wmidi::MidiMessage::NoteOff(_, note, vel) = wmidi_msg {
                    if let Some(msg) = active_midi_notes[u8::from(note) as usize].last_mut() {
                        if let wmidi::MidiMessage::NoteOn(_, _note, old_vel) = &mut msg.1 {
                            *old_vel = U7::from_u8_lossy(128 - u8::from(vel));
                        }
                    }
                }
            });

        let coef = 10f32.powf((*ports.gain).min(90f32).max(-90f32) * 0.05);

        // respond to notes and play sound
        for out_frame in ports.output.iter_mut() {
            *out_frame = 0f32;

            'frame_compute: for pitch_index in 0..active_midi_notes.len() {
                for active_note_index in (0..active_midi_notes[pitch_index].len()).rev() {
                    let since_start = inner.studied_samples.load(Ordering::SeqCst)
                        - active_midi_notes[pitch_index][active_note_index].0;
                    if since_start >= plot_length {
                        active_midi_notes[pitch_index].remove(active_note_index);
                    } else {
                        let chunk_i = since_start / CHUNK_SAMPLE_COUNT;
                        let Ok(chunk) = inner.sum_cache[chunk_i].data.try_lock() else {
                            break 'frame_compute;
                        };

                        let msg = &active_midi_notes[pitch_index][active_note_index].1;
                        let intensity = if let wmidi::MidiMessage::NoteOn(_, _note, vel) = msg {
                            10f32.powf(u8::from(*vel) as f32 / 128f32)
                        } else {
                            1f32
                        };

                        *out_frame +=
                            chunk[since_start % CHUNK_SAMPLE_COUNT][pitch_index] * coef * intensity;
                    }
                }
            }
            inner.studied_samples.fetch_add(1, Ordering::SeqCst);
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

fn compute_cache(synth: Arc<Synth>, recompute: &Receiver<()>, interrupt: Arc<AtomicBool>) {
    // TODO: clear frequency cache if frequency_count or frequenvy_divisions_per_semitone change
    let mut frequency_cache =
        vec![0f32; synth.data.read_with(|data| data.frequency_count as usize)];
    let mut midi_frequency_cache = vec![0f32; PLAUSIBLE_MIDI_FREQUENCIES];
    let sample_recip = 1f32 / synth.sample_rate as f32;

    loop {
        recompute.recv().unwrap();
        let recompute_started = Instant::now();

        synth.data.read_with(|data| {
            for (chunk_i, chunk_wrapper) in synth.sum_cache.iter().enumerate() {
                let invalid = chunk_wrapper.invalidate.load(Ordering::SeqCst);
                if !invalid {
                    continue;
                }
                let Ok(mut chunk) = chunk_wrapper.data.try_lock() else {
                    continue;
                };

                let offset = chunk_i * CHUNK_SAMPLE_COUNT;
                for sample_i in offset..(offset + CHUNK_SAMPLE_COUNT).min(data.plot_length) {
                    //TODO: proper midi range selection
                    for midi_note in 48..84 {
                        let midi_freq = {
                            let ret = &mut midi_frequency_cache[midi_note];
                            if *ret == 0f32 {
                                *ret = Note::from_u8_lossy(midi_note as u8).to_freq_f32();
                            }
                            *ret
                        };
                        let mut sum = 0f32;
                        for freq_i in 0..data.frequency_count {
                            let base_freq = {
                                let ret = &mut frequency_cache[freq_i];
                                if *ret == 0f32 {
                                    *ret = 2f32.powf(
                                        (freq_i as i32 - data.frequency_count as i32 / 2) as f32
                                            / (12 * data.frequency_divisions_per_semitone) as f32,
                                    );
                                }
                                *ret
                            };
                            let frequency = midi_freq * base_freq;

                            let amplitude =
                                data.amplitude_plot[freq_i * data.plot_length + sample_i];

                            if amplitude > 0f32 {
                                sum += (sample_i as f32
                                    * sample_recip as f32
                                    * frequency
                                    * 2f32
                                    * f32::consts::PI)
                                    .sin()
                                    * amplitude;
                            }
                        }
                        if interrupt.load(Ordering::SeqCst) {
                            return;
                        }
                        chunk[sample_i % CHUNK_SAMPLE_COUNT][midi_note as usize] = sum;
                    }
                }
                chunk_wrapper.invalidate.store(false, Ordering::SeqCst);
            }
        });
        println!(
            "done with recompute in {} ms",
            recompute_started.elapsed().as_millis()
        );
    }
}
