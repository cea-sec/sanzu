use anyhow::{Context, Result};
use byteorder::{LittleEndian, WriteBytesExt};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Sample;
use cpal::SampleRate;
use sanzu_common::tunnel;
use std::{
    collections::VecDeque,
    io::Cursor,
    sync::{mpsc, Arc, Mutex},
    thread,
    time::Duration,
};

use spin_sleep::sleep;

/// Sampling frequency
pub const SOUND_FREQ: u32 = 48000;
pub const DECODER_BUFFER_MS: usize = 150;
pub const TARGET_SAMPLE_RATE: u32 = 48000;

fn enqueue_from_source<T, U>(
    input: &[T],
    buffering_queue: Arc<Mutex<(usize, VecDeque<Vec<i16>>)>>,
    max_buffer_ms: u64,
) where
    T: cpal::Sample,
    U: cpal::Sample + hound::Sample + cpal::FromSample<T>,
{
    let mut data = vec![];
    for &sample in input.iter() {
        let sample: U = U::from_sample(sample);
        data.push(sample.as_i16());
    }

    let mut buffering_queue_guard = buffering_queue.lock().unwrap();

    buffering_queue_guard.0 += data.len();
    buffering_queue_guard.1.push_back(data);

    let sound_max_len: usize = (max_buffer_ms as usize * SOUND_FREQ as usize * 2) / 1000;

    while buffering_queue_guard.0 > sound_max_len {
        let tmp = buffering_queue_guard
            .1
            .pop_front()
            .expect("Should not be here");
        buffering_queue_guard.0 -= tmp.len();
    }
}

/// Initialize a sound encoder
///
/// `sender` - a mpsc::Sender which will receive encoded sound samples
pub fn init_sound_encoder(
    device_name: &str,
    buffering_queue: Arc<Mutex<(usize, VecDeque<Vec<i16>>)>>,
    sample_rate: u32,
    max_buffer_ms: u64,
) -> Result<cpal::Stream> {
    /* Sound */
    // Conditionally compile with jack if the feature is specified.
    let host = cpal::default_host();

    // Setup the input device and stream with the default input config.
    let device = if device_name == "default" {
        host.default_input_device()
    } else {
        host.input_devices()?
            .find(|x| x.name().map(|y| y == device_name).unwrap_or(false))
    }
    .context("Error in finding sound input device")?;

    debug!("Input device: {:?}", device.name());

    let default_config = device
        .default_input_config()
        .context("Error in get default sound input config")?;
    debug!("Default input config: {:?}", default_config);

    let configs = device
        .supported_output_configs()
        .context("Error in get sound input config")?;
    let mut selected_config = None;
    for config in configs {
        debug!("config {:?} {}", config, sample_rate);
        if config.channels() != default_config.channels() {
            continue;
        }

        if config.sample_format() != default_config.sample_format() {
            continue;
        }

        if sample_rate < config.min_sample_rate().0 {
            continue;
        }

        if sample_rate > config.max_sample_rate().0 {
            continue;
        }
        let config = config.with_sample_rate(SampleRate(sample_rate));
        selected_config = Some(config);
        break;
    }
    let config = selected_config.context("No suitable sound context")?;

    // A flag to indicate that recording is in progress.

    let err_fn = move |err| {
        error!("An error occurred on stream: {}", err);
    };

    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => device
            .build_input_stream(
                &config.into(),
                move |data, _: &_| {
                    enqueue_from_source::<f32, i16>(data, buffering_queue.clone(), max_buffer_ms)
                },
                err_fn,
                None,
            )
            .context("Error in build_input_stream")?,
        cpal::SampleFormat::I16 => device
            .build_input_stream(
                &config.into(),
                move |data, _: &_| {
                    enqueue_from_source::<i16, i16>(data, buffering_queue.clone(), max_buffer_ms)
                },
                err_fn,
                None,
            )
            .context("Error in build_input_stream")?,
        cpal::SampleFormat::U16 => device
            .build_input_stream(
                &config.into(),
                move |data, _: &_| {
                    enqueue_from_source::<u16, i16>(data, buffering_queue.clone(), max_buffer_ms)
                },
                err_fn,
                None,
            )
            .context("Error in build_input_stream")?,
        sample_format => {
            return Err(anyhow::Error::msg(format!(
                "Unsupported sample format '{sample_format}'"
            )))
        }
    };

    Ok(stream)
}

/// Holds SoundDecoder information
pub struct SoundDecoder {
    /// Decoder stream
    stream: cpal::Stream,
    /// Sample rate
    pub sample_rate: u32,
    /// Source of encoded packets
    pkt_q: Arc<Mutex<VecDeque<Vec<u8>>>>,
}

impl SoundDecoder {
    pub fn new(
        device_name: &str,
        sample_rate: Option<u32>,
        audio_buffer_ms: u32,
    ) -> Result<SoundDecoder> {
        let mut decoder = opus::Decoder::new(SOUND_FREQ, opus::Channels::Mono)
            .expect("Cannot create sound decoder");
        let pkt_q: Arc<Mutex<VecDeque<Vec<u8>>>> = Arc::new(Mutex::new(VecDeque::new()));
        let sound_q = Arc::new(Mutex::new(VecDeque::new()));

        let sound_queue_cp = sound_q.clone();
        let pkt_q_cp = pkt_q.clone();

        thread::spawn(move || {
            let mut output = vec![0i16; 100000];
            loop {
                while let Some(pkt) = pkt_q_cp.lock().unwrap().pop_front() {
                    if let Ok(len) = decoder.decode(&pkt, &mut output, false) {
                        trace!("Sound: decoded {:?} {:?}", pkt.len(), len);
                        let real_output = output[0..len].to_owned();
                        let mut data: std::collections::VecDeque<i16> =
                            real_output.into_iter().collect();
                        sound_q.lock().unwrap().append(&mut data);
                        {
                            // Forget old datas if we are lagging more than audio_buffer_ms
                            let sound_max_len =
                                (audio_buffer_ms as usize * SOUND_FREQ as usize * 2) / 1000;
                            let mut sound_data = sound_q.lock().unwrap();
                            if sound_data.len() > sound_max_len {
                                debug!("Sound queue too big");
                                let diff = sound_data.len() - sound_max_len;
                                let _drained = sound_data.drain(0..diff).collect::<VecDeque<_>>();
                            }
                        }
                    }
                }
                // Wait for buffer loading
                sleep(Duration::from_millis(10));
            }
        });

        /* Sound */
        let host = cpal::default_host();

        let device = if device_name == "default" {
            host.default_output_device()
        } else {
            host.output_devices()
                .context("Error in sound output devices")?
                .find(|x| x.name().map(|y| y == device_name).unwrap_or(false))
        }
        .expect("failed to find output device");
        debug!("Output device: {:?}", device.name()?);

        let default_config = device.default_output_config().unwrap();

        let configs = device
            .supported_output_configs()
            .expect("Cannot get output config");

        debug!("Default config {:?}", default_config);
        let mut selected_config = None;
        for config in configs {
            trace!("config {:?} {:?}", config, sample_rate);
            if config.channels() != default_config.channels() {
                continue;
            }

            if config.sample_format() != default_config.sample_format() {
                continue;
            }

            let sample_rate_min = config.min_sample_rate().0;
            let sample_rate_max = config.max_sample_rate().0;

            if let Some(sample_rate) = sample_rate {
                if sample_rate < sample_rate_min {
                    continue;
                }

                if sample_rate > sample_rate_max {
                    continue;
                }

                // Force sample rate
                selected_config = Some(config.with_sample_rate(SampleRate(sample_rate)));
                break;
            }

            let selected_sample_rate = TARGET_SAMPLE_RATE.clamp(sample_rate_min, sample_rate_max);
            selected_config = Some(config.with_sample_rate(SampleRate(selected_sample_rate)));
            break;
        }

        let config = selected_config.expect("No suitable sound config");
        let sample_rate = config.sample_rate().0;

        debug!("Default output config: {:?}", config);

        let sample_format = config.sample_format();
        let config_in: cpal::StreamConfig = config.into();

        let channels = config_in.channels as usize;

        let err_fn = |err| error!("An error occurred on stream: {}", err);

        let stream = match sample_format {
            cpal::SampleFormat::F32 => device
                .build_output_stream(
                    &config_in,
                    move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                        dequeue_to_sink(data, channels, &sound_queue_cp, audio_buffer_ms)
                    },
                    err_fn,
                    None,
                )
                .context("Error in build_output_stream")?,
            cpal::SampleFormat::I16 => device
                .build_output_stream(
                    &config_in,
                    move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                        dequeue_to_sink(data, channels, &sound_queue_cp, audio_buffer_ms)
                    },
                    err_fn,
                    None,
                )
                .context("Error in build_output_stream")?,
            cpal::SampleFormat::U16 => device
                .build_output_stream(
                    &config_in,
                    move |data: &mut [u16], _: &cpal::OutputCallbackInfo| {
                        dequeue_to_sink(data, channels, &sound_queue_cp, audio_buffer_ms)
                    },
                    err_fn,
                    None,
                )
                .context("Error in build_output_stream")?,
            sample_format => {
                return Err(anyhow::Error::msg(format!(
                    "Unsupported sample format '{sample_format}'"
                )))
            }
        };

        Ok(SoundDecoder {
            stream,
            sample_rate,
            pkt_q,
        })
    }

    pub fn start(&mut self) -> Result<()> {
        self.stream.play().context("Error in stream play")
    }

    pub fn push(&mut self, data: Vec<u8>) {
        self.pkt_q.lock().unwrap().push_back(data);
    }
}

fn dequeue_to_sink<T>(
    output: &mut [T],
    channels: usize,
    sound_data: &Arc<Mutex<VecDeque<i16>>>,
    audio_buffer_ms: u32,
) where
    T: cpal::Sample + cpal::FromSample<i16>,
{
    let mut count = 0;
    let mut count_bad = 0;
    let mut sound_data_in = sound_data.lock().unwrap();
    for frame in output.chunks_mut(channels) {
        count += 1;
        for sample in frame.iter_mut() {
            let value = if let Some(data) = sound_data_in.pop_front() {
                data
            } else {
                count_bad += 1;
                // Default sound data
                0
            };

            let value = value.to_sample();
            *sample = value;
        }
    }
    if count_bad != 0 {
        debug!("Missing sound: data: {} miss: {}", count, count_bad);
        /* Enqueue dummy data. The goal is to let a chance to the server to give
         * us fresh data and avoid being continuously near the end of the
         * buffer. If the server finally send us data, it will be enqueued and
         * push out this dummy data.
         */
        let initial_buf_len = (audio_buffer_ms as usize / 4 * SOUND_FREQ as usize * 2) / 1000;
        for _ in 0..initial_buf_len {
            sound_data_in.push_back(0);
        }
    }
}

/// Holds SoundEncoder information
///
/// - the Sound encoder receives raw sound and stores it to the buffer queue
/// `buffering_queue`
/// - data in the buffer queue is polled by `read_sound` from there and sent to
/// the `sound_buffer`
/// - this data is compressed in the separated thread and compressed data is
/// serialized into messages
/// - those messages are retrieved by polling `recv_events`
///
/// You may notice that the `sound_buffer` may be avoided. This designed is done
/// to apply a possible 'back pressure' on the data received. If the client is
/// stalled (for example by network lag), the `read_sound` is not called. In
/// this case, the queue continue to be updated with the very fresh sound, but
/// `sound_buffer` remains empty (it has been emptied by the encoding
/// thread). When the network comes back, the client code loop restarts and
/// calls `read_sound`. The queue buffer `buffering_queue` is then emptied in
/// the `sound_buffer` and the fresh sound is encoded and sent to the
/// client. The sound restart at the client side.
pub struct SoundEncoder {
    /// Decoder stream
    stream: cpal::Stream,
    buffering_queue: Arc<Mutex<(usize, VecDeque<Vec<i16>>)>>,
    sound_buffer: Arc<Mutex<VecDeque<Vec<i16>>>>,
    /// Receiver of compressed and encoded sound messages
    events_receiver: mpsc::Receiver<tunnel::MessageSrv>,
}

// Encode raw sound and serialize it.
//
// It may be possible that we don't have enough data to encode sound. In this
// case, return None
pub fn encode_sound(
    encoder: &mut opus::Encoder,
    sound_data: &mut Vec<i16>,
) -> Option<tunnel::MessageSrv> {
    const MONO_20MS: usize = SOUND_FREQ as usize * 2 * 20 / 1000;
    let mut sound_data_encoded = vec![];
    while sound_data.len() > MONO_20MS {
        let input: Vec<i16> = sound_data.drain(0..MONO_20MS).collect();
        let mut output = vec![0u8; 10000];
        if let Ok(len) = encoder.encode(&input, &mut output) {
            let output = output[0..len].to_owned();
            sound_data_encoded.push(output);
        } else {
            error!("Sound: Cannot encode");
        }
    }

    if !sound_data_encoded.is_empty() {
        let sound_event = tunnel::EventSoundEncoded {
            data: sound_data_encoded,
        };

        // Push sound event
        let sound_event = tunnel::message_srv::Msg::SoundEncoded(sound_event);
        let sound_event = tunnel::MessageSrv {
            msg: Some(sound_event),
        };
        Some(sound_event)
    } else {
        None
    }
}

impl SoundEncoder {
    pub fn new(
        device_name: &str,
        raw_sound: bool,
        sample_rate: u32,
        max_buffer_ms: u64,
    ) -> Result<SoundEncoder> {
        let buffering_queue = Arc::new(Mutex::new((0, VecDeque::new())));
        let sound_buffer = Arc::new(Mutex::new(VecDeque::new()));
        let (events_sender, events_receiver): (
            mpsc::Sender<tunnel::MessageSrv>,
            mpsc::Receiver<tunnel::MessageSrv>,
        ) = mpsc::channel();

        let mut encoder = match raw_sound {
            true => None,
            false => Some(
                opus::Encoder::new(
                    SOUND_FREQ,
                    opus::Channels::Mono,
                    opus::Application::LowDelay,
                )
                .expect("Cannot create sound encoder"),
            ),
        };

        let stream = init_sound_encoder(
            device_name,
            buffering_queue.clone(),
            sample_rate,
            max_buffer_ms,
        )?;
        let sound_buffer_cp = sound_buffer.clone();
        thread::spawn(move || {
            let mut sound_data = vec![];
            let mut need_data = true;
            loop {
                /* Receive sound */
                {
                    let mut sound_buffer_guard = sound_buffer_cp.lock().unwrap();
                    while let Some(mut data) = sound_buffer_guard.pop_front() {
                        sound_data.append(&mut data);
                        need_data = false;
                    }
                }

                if need_data {
                    sleep(Duration::from_millis(10));
                    continue;
                }
                match &mut encoder {
                    Some(ref mut encoder) => {
                        if let Some(sound_event) = encode_sound(encoder, &mut sound_data) {
                            events_sender
                                .send(sound_event)
                                .expect("Cannot send encoded data sound");
                        } else {
                            need_data = true;
                        }
                    }
                    None => {
                        if !sound_data.is_empty() {
                            let data_raw: Vec<u8> = vec![0u8; sound_data.len() * 2];
                            let mut wtr = Cursor::new(data_raw);

                            for sample in sound_data.iter() {
                                wtr.write_i16::<LittleEndian>(*sample).unwrap();
                            }
                            sound_data.clear();

                            let sound_event = tunnel::EventSoundRaw {
                                data: wtr.get_ref().to_owned(),
                            };

                            // Push sound event
                            let sound_event = tunnel::message_srv::Msg::SoundRaw(sound_event);
                            let sound_event = tunnel::MessageSrv {
                                msg: Some(sound_event),
                            };

                            events_sender
                                .send(sound_event)
                                .expect("Cannot send encoded data sound");
                        }
                    }
                }
                // Wait for buffer loading
                thread::sleep(Duration::from_millis(2));
            }
        });

        Ok(SoundEncoder {
            stream,
            buffering_queue,
            sound_buffer,
            events_receiver,
        })
    }

    // Transfer data from the raw sound buffer queue into the raw sound buffer
    pub fn read_sound(&mut self) {
        let mut buffering_queue_guard = self.buffering_queue.lock().unwrap();
        let mut sound_buffer_guard = self.sound_buffer.lock().unwrap();
        while let Some(data) = buffering_queue_guard.1.pop_front() {
            buffering_queue_guard.0 -= data.len();
            sound_buffer_guard.push_back(data)
        }
    }

    // Retrieve compressed sound packets
    pub fn recv_events(&mut self) -> Vec<tunnel::MessageSrv> {
        let mut events = vec![];
        /* Receive sound */
        while let Ok(event) = self.events_receiver.try_recv() {
            events.push(event);
        }

        events
    }

    // Start sound recording
    pub fn start(&mut self) -> Result<()> {
        self.stream.play().context("Error in stream play")
    }
}
