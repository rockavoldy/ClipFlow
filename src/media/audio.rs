//! WASAPI loopback + microphone capture, mixed and AAC-encoded into the same
//! rolling ring buffer that holds the video packets.
//!
//! Drift avoidance
//! ---------------
//! WASAPI hands every packet a QPC timestamp taken by the audio engine itself.
//! We convert that stamp with exactly the same [`QpcClock`] epoch the capture
//! thread uses for video, so A/V share one monotonic timeline. No resampling
//! guesswork, no "add 40 ms and hope" - the muxer just writes the stamps.
//!
//! The mic is mixed into the loopback stream in the float domain with a -3 dB
//! pad on both sources to avoid inter-sample clipping. The mix balance is
//! user-controlled: `game_gain`/`mic_gain` carry the permille volumes (0-1000)
//! set in Settings, read fresh on every loop so changes apply live without an
//! engine restart.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;

use super::recorder::{EncodedPacket, RecorderError, RollingRingBuffer, TrackKind, HNS_PER_SECOND};

pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: u32 = 2;
pub const AAC_BYTES_PER_SECOND: u32 = 24_000; // 192 kbit/s stereo

/// AAC-LC decoder configuration for 48 kHz stereo, as stored in the MP4's
/// `esds`/`MF_MT_USER_DATA` blob. The MPEG-4 sink writer needs this to
/// initialise the decoder; without it players decode the raw frames as noise.
pub fn aac_decoder_config() -> Vec<u8> {
    // HEAACWAVEINFO header (12 bytes) + AudioSpecificConfig (2 bytes).
    let mut buf = vec![0u8; 14];
    // wAudioProfileLevelIndication = 0x29 (AAC-LC L2)
    buf[2] = 0x29;
    buf[3] = 0x00;
    // AudioSpecificConfig: AAC-LC (2), 48 kHz (3), 2 channels (2)
    // 0001 0001 1001 0000
    buf[12] = 0x11;
    buf[13] = 0x90;
    buf
}

/// Linear-interpolation stereo resampler (device rate ➜ [`SAMPLE_RATE`]).
///
/// Only used when WASAPI refused the explicit 48 kHz request and capture fell
/// back to the mix format (44.1 kHz USB gear, loopback on strict drivers).
/// The preferred path is still engine-side conversion; this keeps clips
/// correct everywhere else. Input and output are both interleaved stereo.
#[cfg_attr(any(not(windows), feature = "headless-sim"), allow(dead_code))]
struct LinearResampler {
    /// Input frames per output frame.
    step: f64,
    /// Fractional input position of the next output frame.
    pos: f64,
    prev: [f32; 2],
    have_prev: bool,
}

#[cfg_attr(any(not(windows), feature = "headless-sim"), allow(dead_code))]
impl LinearResampler {
    fn new(input_rate: u32) -> Self {
        Self {
            step: input_rate.max(1) as f64 / SAMPLE_RATE as f64,
            pos: 0.0,
            prev: [0.0, 0.0],
            have_prev: false,
        }
    }

    fn resample(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if input.len() < 2 {
            return;
        }
        let frames = input.len() / 2;
        // Out-of-range taps hold the edge sample, which keeps the stream
        // continuous across batch boundaries (`prev` bridges the gap).
        let at = |i: isize| -> [f32; 2] {
            if i < 0 {
                if self.have_prev {
                    self.prev
                } else {
                    [input[0], input[1]]
                }
            } else if (i as usize) >= frames {
                [input[frames * 2 - 2], input[frames * 2 - 1]]
            } else {
                [input[i as usize * 2], input[i as usize * 2 + 1]]
            }
        };
        out.reserve((frames as f64 / self.step) as usize * 2 + 4);
        while self.pos <= frames as f64 - 1.0 {
            let i0 = self.pos.floor() as isize;
            let frac = (self.pos - i0 as f64) as f32;
            let a = at(i0);
            let b = at(i0 + 1);
            out.push(a[0] + (b[0] - a[0]) * frac);
            out.push(a[1] + (b[1] - a[1]) * frac);
            self.pos += self.step;
        }
        self.pos -= frames as f64;
        self.prev = [input[frames * 2 - 2], input[frames * 2 - 1]];
        self.have_prev = true;
    }
}

/// The fixed −3 dB pad applied per source when loopback and microphone are
/// mixed together, so their sum cannot clip.
#[cfg_attr(any(not(windows), feature = "headless-sim"), allow(dead_code))]
const MIX_PAD: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// Largest mic backlog kept, in interleaved samples (200 ms). The render and
/// microphone endpoints have independent clocks; if the mic runs slightly fast
/// its queue must be trimmed somewhere instead of growing for the session.
#[cfg_attr(any(not(windows), feature = "headless-sim"), allow(dead_code))]
const MAX_MIC_BACKLOG: usize = SAMPLE_RATE as usize * CHANNELS as usize / 5;

/// How many consecutive 8 ms polls may have no loopback packet before the
/// microphone is allowed to drive the timeline on its own (~256 ms).
#[cfg_attr(any(not(windows), feature = "headless-sim"), allow(dead_code))]
const MAX_IDLE_LOOPBACK_POLLS: u32 = 32;

/// Appends one poll's captured samples to a per-source queue, applying the live
/// gain and the fixed mix pad exactly once.
#[cfg_attr(any(not(windows), feature = "headless-sim"), allow(dead_code))]
fn push_source(queue: &mut Vec<f32>, batch: &[f32], gain: f32, pad: f32) {
    let scale = gain * pad;
    queue.extend(batch.iter().map(|s| (*s * scale).clamp(-1.0, 1.0)));
}

/// Mixes the mic over the loopback stream, which is the timeline.
///
/// **Never** produces a sample for mic data that has no loopback partner. The
/// endpoints deliver packets on independent cadences (both roughly 10 ms, but
/// offset), so a poll often has one and not the other. Appending the mic tail
/// in that case inserted ~180 samples of digital silence into the game audio
/// several times a second — heard as continuous crackle whenever the
/// microphone was enabled.
#[cfg_attr(any(not(windows), feature = "headless-sim"), allow(dead_code))]
fn mix_into_loopback(pending: &mut Vec<f32>, loopback: &mut Vec<f32>, mic: &mut Vec<f32>) {
    let take = loopback.len();
    if take == 0 {
        // No game audio this poll: keep the mic queued for the next one.
        return;
    }
    let mic_take = take.min(mic.len());
    pending.reserve(take);
    for i in 0..take {
        let m = if i < mic_take { mic[i] } else { 0.0 };
        pending.push((loopback[i] + m).clamp(-1.0, 1.0));
    }
    loopback.drain(..take);
    if mic_take > 0 {
        mic.drain(..mic_take);
    }
}

/// Drops the oldest mic samples when its clock outruns the loopback clock, so
/// a long session cannot accumulate an ever-growing mic delay.
#[cfg_attr(any(not(windows), feature = "headless-sim"), allow(dead_code))]
fn bound_mic_backlog(mic: &mut Vec<f32>) {
    if mic.len() <= MAX_MIC_BACKLOG {
        return;
    }
    let mut drop = mic.len() - MAX_MIC_BACKLOG;
    // Keep whole stereo frames so channels never swap.
    drop -= drop % CHANNELS as usize;
    if drop > 0 {
        mic.drain(..drop);
    }
}

/// Decodes packed little-endian 24-bit PCM to floats. Reaches the `else`
/// branch of the WASAPI decoder on 24-bit mix formats (common on USB DACs,
/// AVRs and TVs); before this the encoder received digital silence.
#[cfg_attr(any(not(windows), feature = "headless-sim"), allow(dead_code))]
fn pcm24_to_f32(bytes: &[u8], out: &mut Vec<f32>) {
    out.reserve(bytes.len() / 3);
    for s in bytes.as_chunks::<3>().0 {
        let raw = (s[0] as i32) | ((s[1] as i32) << 8) | ((s[2] as i32) << 16);
        // Sign-extend the 24-bit value before scaling.
        let value = (raw << 8) >> 8;
        out.push(value as f32 / 8_388_608.0);
    }
}

pub struct AudioCapture {
    stop: Arc<AtomicBool>,
    handles: Vec<std::thread::JoinHandle<()>>,
}

impl AudioCapture {
    /// `status` is the engine's shared audio-error slot: the thread writes the
    /// reason it cannot capture (endpoint busy, AAC missing...) so the UI can
    /// show *why* clips come out without audio instead of guessing.
    pub fn start(
        system: bool,
        mic: bool,
        game_gain: Arc<AtomicU32>,
        mic_gain: Arc<AtomicU32>,
        ring: Arc<Mutex<RollingRingBuffer>>,
        generation: Arc<AtomicU32>,
        status: Arc<Mutex<Option<String>>>,
        qpc_origin: i64,
        qpc_freq: i64,
    ) -> Result<Self, RecorderError> {
        let stop = Arc::new(AtomicBool::new(false));
        let mut handles = Vec::new();

        if system || mic {
            let stop_c = Arc::clone(&stop);
            let gg_c = Arc::clone(&game_gain);
            let mg_c = Arc::clone(&mic_gain);
            let ring_c = Arc::clone(&ring);
            let gen_c = Arc::clone(&generation);
            let status_c = Arc::clone(&status);
            let h = std::thread::Builder::new()
                .name("yascreenclipper-audio".into())
                .spawn(move || {
                    imp::run(system, mic, gg_c, mg_c, stop_c, ring_c, gen_c, status_c, qpc_origin, qpc_freq);
                })
                .map_err(|e| RecorderError::Other(format!("audio thread: {e}")))?;
            handles.push(h);
        }

        Ok(Self { stop, handles })
    }

    pub fn stop(self) {
        self.stop.store(true, Ordering::Release);
        for h in self.handles {
            let _ = h.join();
        }
    }
}

// ---------------------------------------------------------------------------
// Windows implementation
// ---------------------------------------------------------------------------

#[cfg(all(windows, not(feature = "headless-sim")))]
mod imp {
    use super::*;
    use windows::Win32::Media::Audio::*;
    use windows::Win32::Media::KernelStreaming::{
        SPEAKER_FRONT_LEFT, SPEAKER_FRONT_RIGHT, WAVE_FORMAT_EXTENSIBLE,
    };
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
        COINIT_MULTITHREADED,
    };

    const KSDATAFORMAT_SUBTYPE_IEEE_FLOAT: windows::core::GUID =
        windows::core::GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);

    struct Endpoint {
        client: IAudioClient,
        capture: IAudioCaptureClient,
        channels: u32,
        _rate: u32,
        float: bool,
        bits: u32,
        /// Software resampler to 48 kHz, used only when the engine refused
        /// the explicit 48 kHz request and capture fell back to the mix
        /// format. `None` on the preferred path (the engine converts).
        resampler: Option<LinearResampler>,
    }

    /// The 48 kHz stereo float format the AAC encoder expects. Requested
    /// first so the engine converts; several drivers (loopback in
    /// particular) reject anything but the mix format, which is what the
    /// fallback in [`open_device`] is for.
    unsafe fn requested_format() -> WAVEFORMATEXTENSIBLE {
        let mut requested = WAVEFORMATEXTENSIBLE::default();
        requested.Format.wFormatTag = WAVE_FORMAT_EXTENSIBLE as u16;
        requested.Format.nChannels = CHANNELS as u16;
        requested.Format.nSamplesPerSec = SAMPLE_RATE;
        requested.Format.wBitsPerSample = 32;
        requested.Format.nBlockAlign = (CHANNELS * 4) as u16;
        requested.Format.nAvgBytesPerSec = SAMPLE_RATE * CHANNELS * 4;
        requested.Format.cbSize = (std::mem::size_of::<WAVEFORMATEXTENSIBLE>()
            - std::mem::size_of::<WAVEFORMATEX>()) as u16;
        requested.Samples.wValidBitsPerSample = 32;
        requested.dwChannelMask = SPEAKER_FRONT_LEFT | SPEAKER_FRONT_RIGHT;
        requested.SubFormat = KSDATAFORMAT_SUBTYPE_IEEE_FLOAT;
        requested
    }

    unsafe fn start_endpoint(
        client: &IAudioClient,
    ) -> windows::core::Result<IAudioCaptureClient> {
        let capture: IAudioCaptureClient = client.GetService()?;
        client.Start()?;
        Ok(capture)
    }

    unsafe fn open_endpoint(loopback: bool) -> windows::core::Result<Endpoint> {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let flow = if loopback { eRender } else { eCapture };

        // Prefer the default device, but fall back to any active endpoint. A
        // disabled or unplugged default device otherwise makes capture
        // impossible even when another device is present.
        let mut last_error = match enumerator.GetDefaultAudioEndpoint(flow, eConsole) {
            Ok(device) => match open_device(&device, loopback) {
                Ok(endpoint) => return Ok(endpoint),
                Err(e) => e,
            },
            Err(e) => e,
        };

        if let Ok(devices) = enumerator.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE) {
            let count = devices.GetCount().unwrap_or(0);
            for index in 0..count {
                let Ok(device) = devices.Item(index) else {
                    continue;
                };
                match open_device(&device, loopback) {
                    Ok(endpoint) => return Ok(endpoint),
                    Err(e) => last_error = e,
                }
            }
        }

        Err(last_error)
    }

    unsafe fn open_device(
        device: &IMMDevice,
        loopback: bool,
    ) -> windows::core::Result<Endpoint> {
        // Capture polls `GetNextPacketSize`; it is not event-driven. Requesting
        // EVENTCALLBACK without calling `SetEventHandle` makes `Start` fail with
        // AUDCLNT_E_EVENTHANDLE_NOT_SET on strict drivers, so the mic endpoint
        // must not ask for it.
        let flags = if loopback {
            // Loopback streams cannot be event driven on all drivers; poll them.
            AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
        } else {
            // Microphone: ask the engine to resample/convert into 48 kHz float
            // (SRC_DEFAULT_QUALITY) so we never depend on the device mix format.
            AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY
        };

        // 100 ms of engine buffering: this thread polls *and* runs the AAC
        // MFT, so a slow encode burst must not overrun capture. A 20 ms
        // buffer dropped packets audibly as gaps and crackle.
        const BUFFER_HNS: i64 = 1_000_000;

        // Loopback capture must use the render endpoint's mix format; drivers
        // reject any other format on a loopback stream even when `Initialize`
        // reports success, which is what produced garbage audio before.
        if loopback {
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
            let mix = client.GetMixFormat()?;
            let (channels, rate, float, bits) = {
                let wf = &*mix;
                let channels = wf.nChannels as u32;
                let rate = wf.nSamplesPerSec;
                let bits = wf.wBitsPerSample as u32;
                let float = if wf.wFormatTag == 0xFFFE {
                    let ext = mix as *const WAVEFORMATEXTENSIBLE;
                    let subformat = std::ptr::addr_of!((*ext).SubFormat).read_unaligned();
                    subformat == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
                } else {
                    wf.wFormatTag == 3
                };
                (channels, rate, float, bits)
            };
            client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, BUFFER_HNS, 0, mix, None)?;
            let capture = start_endpoint(&client)?;
            CoTaskMemFree(Some(mix as *const std::ffi::c_void));

            let resampler = (rate != SAMPLE_RATE).then(|| LinearResampler::new(rate));
            return Ok(Endpoint {
                client,
                capture,
                channels,
                _rate: rate,
                float,
                bits,
                resampler,
            });
        }

        // Microphone: prefer an explicit 48 kHz stereo float request so the
        // engine converts. Pass the full extensible struct (not just its
        // WAVEFORMATEX header) — the previous code passed a copied header and
        // the engine read garbage past the end of it.
        let requested = requested_format();
        let explicit: windows::core::Result<Endpoint> = (|| {
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                flags,
                BUFFER_HNS,
                0,
                &requested.Format,
                None,
            )?;
            let capture = start_endpoint(&client)?;
            Ok(Endpoint {
                client,
                capture,
                channels: CHANNELS,
                _rate: SAMPLE_RATE,
                float: true,
                bits: 32,
                resampler: None,
            })
        })();
        if let Ok(endpoint) = explicit {
            return Ok(endpoint);
        }

        // Fallback: the mix format always initialises. The decoded stereo is
        // then resampled to 48 kHz in software (`resampler`), which is what
        // keeps 44.1 kHz devices at the right pitch and in A/V sync.
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
        let mix = client.GetMixFormat()?;
        let (channels, rate, float, bits) = {
            let wf = &*mix;
            let channels = wf.nChannels as u32;
            let rate = wf.nSamplesPerSec;
            let bits = wf.wBitsPerSample as u32;
            let float = if wf.wFormatTag == 0xFFFE {
                let ext = mix as *const WAVEFORMATEXTENSIBLE;
                let subformat = std::ptr::addr_of!((*ext).SubFormat).read_unaligned();
                subformat == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
            } else {
                wf.wFormatTag == 3
            };
            (channels, rate, float, bits)
        };
        client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, BUFFER_HNS, 0, mix, None)?;
        let capture = start_endpoint(&client)?;
        CoTaskMemFree(Some(mix as *const std::ffi::c_void));

        let resampler = (rate != SAMPLE_RATE).then(|| LinearResampler::new(rate));
        Ok(Endpoint {
            client,
            capture,
            channels,
            _rate: rate,
            float,
            bits,
            resampler,
        })
    }

    unsafe fn drain_endpoint(ep: &mut Endpoint, out: &mut Vec<f32>, qpc_first: &mut Option<i64>) {
        // Decoded stereo at the *endpoint* rate; resampled to 48 kHz below
        // when the mix-format fallback is in play.
        let mut native: Vec<f32> = Vec::new();
        loop {
            let packet = match ep.capture.GetNextPacketSize() {
                Ok(p) => p,
                Err(_) => return,
            };
            if packet == 0 {
                break;
            }
            let mut data: *mut u8 = std::ptr::null_mut();
            let mut frames = 0u32;
            let mut flags = 0u32;
            let mut dev_pos = 0u64;
            let mut qpc = 0u64;
            if ep
                .capture
                .GetBuffer(
                    &mut data,
                    &mut frames,
                    &mut flags,
                    Some(&mut dev_pos),
                    Some(&mut qpc),
                )
                .is_err()
            {
                break;
            }
            if qpc_first.is_none() && qpc != 0 {
                *qpc_first = Some(qpc as i64);
            }

            let silent = (flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0;
            let n = frames as usize * ep.channels as usize;
            // Silent and unknown-encoding packets are stereo zeros: the
            // downstream mix and the AAC encoder both assume interleaved
            // stereo (`CHANNELS`), never the endpoint's native layout.
            if silent || data.is_null() {
                native.extend(std::iter::repeat(0.0).take(frames as usize * CHANNELS as usize));
            } else if ep.float {
                let src = std::slice::from_raw_parts(data as *const f32, n);
                downmix(src, ep.channels, &mut native);
            } else if ep.bits == 16 {
                let src = std::slice::from_raw_parts(data as *const i16, n);
                let tmp: Vec<f32> = src.iter().map(|s| *s as f32 / 32768.0).collect();
                downmix(&tmp, ep.channels, &mut native);
            } else if ep.bits == 24 {
                // Packed 24-bit PCM, no container padding.
                let bytes = std::slice::from_raw_parts(data as *const u8, n * 3);
                let mut tmp = Vec::with_capacity(n);
                pcm24_to_f32(bytes, &mut tmp);
                downmix(&tmp, ep.channels, &mut native);
            } else if ep.bits == 32 {
                // 32-bit integer (and 24-valid-bits-in-32, which is
                // left-justified, so the same scale is correct).
                let src = std::slice::from_raw_parts(data as *const i32, n);
                let tmp: Vec<f32> = src
                    .iter()
                    .map(|s| *s as f32 / 2_147_483_648.0)
                    .collect();
                downmix(&tmp, ep.channels, &mut native);
            } else {
                native.extend(std::iter::repeat(0.0).take(frames as usize * CHANNELS as usize));
            }
            let _ = ep.capture.ReleaseBuffer(frames);
        }
        match ep.resampler.as_mut() {
            Some(rs) => rs.resample(&native, out),
            None => out.extend_from_slice(&native),
        }
    }

    /// Any channel layout ➜ interleaved stereo.
    fn downmix(src: &[f32], channels: u32, out: &mut Vec<f32>) {
        match channels {
            1 => {
                for s in src {
                    out.push(*s);
                    out.push(*s);
                }
            }
            2 => out.extend_from_slice(src),
            n => {
                let n = n as usize;
                for frame in src.chunks_exact(n) {
                    // L = FL + 0.707*C + 0.707*SL, R = FR + 0.707*C + 0.707*SR
                    let c = frame.get(2).copied().unwrap_or(0.0) * 0.707;
                    let l = frame[0] + c + frame.get(4).copied().unwrap_or(0.0) * 0.707;
                    let r = frame[1] + c + frame.get(5).copied().unwrap_or(0.0) * 0.707;
                    out.push(l.clamp(-1.0, 1.0));
                    out.push(r.clamp(-1.0, 1.0));
                }
            }
        }
    }

    /// Media Foundation AAC encoder MFT wrapper (PCM16 stereo 48 k ➜ AAC-LC).
    struct AacEncoder {
        transform: IMFTransform,
    }

    impl AacEncoder {
        unsafe fn new() -> windows::core::Result<Self> {
            let transform: IMFTransform = CoCreateInstance(
                &AACMFTEncoder,
                None,
                windows::Win32::System::Com::CLSCTX_INPROC_SERVER,
            )?;

            let in_type = MFCreateMediaType()?;
            in_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
            in_type.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
            in_type.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, SAMPLE_RATE)?;
            in_type.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, CHANNELS)?;
            in_type.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
            in_type.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, CHANNELS * 2)?;
            in_type.SetUINT32(
                &MF_MT_AUDIO_AVG_BYTES_PER_SECOND,
                SAMPLE_RATE * CHANNELS * 2,
            )?;

            let out_type = MFCreateMediaType()?;
            out_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
            out_type.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)?;
            out_type.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, SAMPLE_RATE)?;
            out_type.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, CHANNELS)?;
            out_type.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
            out_type.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, AAC_BYTES_PER_SECOND)?;
            out_type.SetUINT32(&MF_MT_AAC_PAYLOAD_TYPE, 0)?;
            out_type.SetUINT32(&MF_MT_AAC_AUDIO_PROFILE_LEVEL_INDICATION, 0x29)?;

            transform.SetOutputType(0, &out_type, 0)?;
            transform.SetInputType(0, &in_type, 0)?;
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;

            Ok(Self { transform })
        }

        unsafe fn encode(
            &self,
            pcm: &[i16],
            pts_hns: i64,
            mut sink: impl FnMut(Vec<u8>),
        ) -> windows::core::Result<()> {
            let bytes = pcm.len() * 2;
            let buffer = MFCreateMemoryBuffer(bytes as u32)?;
            let mut ptr: *mut u8 = std::ptr::null_mut();
            buffer.Lock(&mut ptr, None, None)?;
            std::ptr::copy_nonoverlapping(pcm.as_ptr() as *const u8, ptr, bytes);
            buffer.Unlock()?;
            buffer.SetCurrentLength(bytes as u32)?;

            let duration = (pcm.len() as i64 / CHANNELS as i64) * HNS_PER_SECOND
                / SAMPLE_RATE as i64;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(pts_hns)?;
            sample.SetSampleDuration(duration)?;
            self.transform.ProcessInput(0, &sample, 0)?;

            loop {
                let mut status = 0u32;
                let info = self.transform.GetOutputStreamInfo(0)?;
                let out_buffer = MFCreateMemoryBuffer(info.cbSize.max(4096))?;
                let out_sample = MFCreateSample()?;
                out_sample.AddBuffer(&out_buffer)?;

                let mut out = [MFT_OUTPUT_DATA_BUFFER {
                    dwStreamID: 0,
                    pSample: std::mem::ManuallyDrop::new(Some(out_sample.clone())),
                    dwStatus: 0,
                    pEvents: std::mem::ManuallyDrop::new(None),
                }];

                match self.transform.ProcessOutput(0, &mut out, &mut status) {
                    Ok(()) => {}
                    Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => {
                        let _ = std::mem::ManuallyDrop::take(&mut out[0].pSample);
                        let _ = std::mem::ManuallyDrop::take(&mut out[0].pEvents);
                        return Ok(());
                    }
                    Err(e) => {
                        let _ = std::mem::ManuallyDrop::take(&mut out[0].pSample);
                        let _ = std::mem::ManuallyDrop::take(&mut out[0].pEvents);
                        return Err(e);
                    }
                }

                let produced = unsafe { std::mem::ManuallyDrop::take(&mut out[0].pSample) };
                let produced = produced.expect("produced sample");
                let contiguous = produced.ConvertToContiguousBuffer()?;
                let mut p: *mut u8 = std::ptr::null_mut();
                let mut cur = 0u32;
                contiguous.Lock(&mut p, None, Some(&mut cur))?;
                let payload = std::slice::from_raw_parts(p, cur as usize).to_vec();
                let _ = contiguous.Unlock();
                let _ = std::mem::ManuallyDrop::take(&mut out[0].pEvents);

                if !payload.is_empty() {
                    // The MFT's output timestamps are unreliable (delayed, duplicated
                    // or zero), so ignore them and use the caller's monotonic PTS.
                    sink(payload);
                }
            }
        }
    }

    /// Turns the common WASAPI failures into something a user can act on. The
    /// raw HRESULTs are otherwise identical-looking hex strings.
    fn endpoint_failure(loopback: bool, e: &windows::core::Error) -> String {
        let device = if loopback {
            "system audio loopback"
        } else {
            "microphone"
        };
        match e.code().0 as u32 {
            0x8007_0005 if loopback => {
                format!("{device} unavailable: access denied (0x80070005)")
            }
            0x8007_0005 => format!(
                "{device} unavailable: access denied — enable Settings > Privacy & security > Microphone > “Let desktop apps access your microphone”"
            ),
            0x8007_0490 => format!("{device} unavailable: no active device found"),
            0x8889_0004 => {
                format!("{device} unavailable: the device was disconnected or disabled")
            }
            0x8889_0008 => {
                format!("{device} unavailable: the device does not support the capture format")
            }
            0x8889_000A => {
                format!("{device} unavailable: the device is in exclusive use by another app")
            }
            0x8889_0017 => {
                format!("{device} unavailable: the audio driver rejected the event handle")
            }
            _ => format!("{device} unavailable: {e}"),
        }
    }

    /// Opens an endpoint, reporting the failure reason into `status`. Returns
    /// `Ok(Some(ep))` on success, `Ok(None)` when the feature is disabled, and
    /// `Err` when the endpoint could not be opened (the shared `status` slot
    /// already carries the human-readable reason).
    unsafe fn open_reported(
        loopback: bool,
        status: &Mutex<Option<String>>,
    ) -> Result<Option<Endpoint>, ()> {
        match open_endpoint(loopback) {
            Ok(ep) => Ok(Some(ep)),
            Err(e) => {
                let msg = endpoint_failure(loopback, &e);
                *status.lock() = Some(msg.clone());
                log::warn!("[yascreenclipper::audio] {msg}");
                Err(())
            }
        }
    }

    pub(super) fn run(
        system: bool,
        mic: bool,
        game_gain: Arc<AtomicU32>,
        mic_gain: Arc<AtomicU32>,
        stop: Arc<AtomicBool>,
        ring: Arc<Mutex<RollingRingBuffer>>,
        generation: Arc<AtomicU32>,
        status: Arc<Mutex<Option<String>>>,
        qpc_origin: i64,
        qpc_freq: i64,
    ) {
        unsafe {
            // This thread both polls WASAPI and runs the AAC MFT, so it must
            // keep its 8 ms cadence while a game saturates the CPU; a starved
            // poll overruns the endpoint buffer and drops packets (audible
            // gaps and A/V drift).
            let _ = windows::Win32::System::Threading::SetThreadPriority(
                windows::Win32::System::Threading::GetCurrentThread(),
                windows::Win32::System::Threading::THREAD_PRIORITY_ABOVE_NORMAL,
            );
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            if MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET).is_err() {
                *status.lock() = Some("Media Foundation failed to start".into());
                CoUninitialize();
                return;
            }

            // Loopback + the AAC encoder are both *retried* forever: a game
            // holding the render device in exclusive mode makes the very first
            // `Initialize` fail with AUDCLNT_E_DEVICE_IN_USE (0x8889000A), and
            // the AAC MFT can briefly be contended on system wake. YAScreenClipper
            // must keep the buffer armed with video while audio sorts itself
            // out, then join seamlessly — never silently produce audio-less
            // clips for the whole session.
            let mut loopback: Option<Endpoint> = None;
            let mut microphone: Option<Endpoint> = None;
            let mut encoder: Option<AacEncoder> = None;
            let mut last_attempt: Option<std::time::Instant> = None;

            let freq = qpc_freq;
            let origin = qpc_origin;
            let freq = if freq == 0 { HNS_PER_SECOND } else { freq };

            // AAC-LC frames are 1024 samples; feed the MFT in exact multiples.
            const FRAME_SAMPLES: usize = 1024 * CHANNELS as usize;
            let mut pending: Vec<f32> = Vec::with_capacity(FRAME_SAMPLES * 8);
            // Per-poll capture batches (reused, so steady-state capture does
            // not allocate) and the un-consumed tail of each source. `pending`
            // is the mixed 48 kHz stream.
            let mut loopback_batch: Vec<f32> = Vec::with_capacity(FRAME_SAMPLES);
            let mut mic_batch: Vec<f32> = Vec::with_capacity(FRAME_SAMPLES);
            let mut loopback_queue: Vec<f32> = Vec::with_capacity(FRAME_SAMPLES);
            let mut mic_queue: Vec<f32> = Vec::with_capacity(FRAME_SAMPLES);
            let mut pcm = Vec::<i16>::with_capacity(FRAME_SAMPLES);
            // Input AAC frames fed to the encoder so far.
            let mut fed_frames: i64 = 0;
            // Output AAC frames written to the ring so far (starts at the
            // input position, so a priming/dropped input leaves a real gap).
            let mut out_frame: i64 = 0;
            let mut qpc_first: Option<i64> = None;
            let mut idle_loopback_polls: u32 = 0;
            let frame_hns = 1024 * HNS_PER_SECOND / SAMPLE_RATE as i64;

            // Fixed pad per source: -3 dB each only when both are configured.
            // A single source stays at unity, and the pad never depends on
            // whether a packet happened to arrive on this particular poll.
            let (loopback_pad, mic_pad) = if system && mic {
                (MIX_PAD, MIX_PAD)
            } else {
                (1.0, 1.0)
            };

            while !stop.load(Ordering::Acquire) {
                // (Re)open endpoints immediately on the first pass, then every
                // second until the device is free. Waiting one full second
                // before the *first* attempt left the first ~1 s of every clip
                // with no audio at all: video capture starts at once, so the
                // first audio packet was stamped a second behind frame zero and
                // the whole track played late (ffprobe: `start 0.987`).
                if last_attempt.is_none_or(|t| t.elapsed() >= std::time::Duration::from_secs(1)) {
                    last_attempt = Some(std::time::Instant::now());
                    if system && loopback.is_none() {
                        match open_reported(true, &status) {
                            Ok(Some(ep)) => loopback = Some(ep),
                            Ok(None) => {}
                            Err(()) => {}
                        }
                    }
                    if mic && microphone.is_none() {
                        match open_reported(false, &status) {
                            Ok(Some(ep)) => microphone = Some(ep),
                            Ok(None) => {}
                            Err(()) => {}
                        }
                    }
                    if encoder.is_none() && (loopback.is_some() || microphone.is_some()) {
                        match AacEncoder::new() {
                            Ok(e) => {
                                encoder = Some(e);
                                *status.lock() = None;
                            }
                            Err(e) => {
                                *status.lock() =
                                    Some(format!("AAC encoder unavailable: {e}"));
                                log::warn!("[yascreenclipper::audio] AAC MFT: {e}");
                            }
                        }
                    }
                }

                if loopback.is_none() && microphone.is_none() {
                    // Nothing captured yet (device busy). Keep the thread alive
                    // and keep retrying; the video side is unaffected.
                    std::thread::sleep(std::time::Duration::from_millis(8));
                    continue;
                }

                loopback_batch.clear();
                if let Some(ep) = loopback.as_mut() {
                    drain_endpoint(ep, &mut loopback_batch, &mut qpc_first);
                }
                mic_batch.clear();
                if let Some(ep) = microphone.as_mut() {
                    drain_endpoint(ep, &mut mic_batch, &mut qpc_first);
                }

                // Apply the mix balance at capture time so gains change live.
                // The pads are fixed for the session. Sources are queued
                // separately and only mixed where the loopback has samples:
                // the loopback stream is the timeline, so a mic packet that
                // lands in a different poll can never inject silence into the
                // game audio or stretch its duration.
                let gg = game_gain.load(Ordering::Acquire) as f32 * 0.001;
                let mg = mic_gain.load(Ordering::Acquire) as f32 * 0.001;
                push_source(&mut loopback_queue, &loopback_batch, gg, loopback_pad);
                push_source(&mut mic_queue, &mic_batch, mg, mic_pad);
                if loopback.is_some() {
                    // A healthy loopback stream delivers packets even when the
                    // system is silent (they carry zeros). If it stops entirely
                    // — default device changed/removed — let the mic drive for
                    // a moment instead of losing it behind the frozen clock.
                    if loopback_queue.is_empty() {
                        idle_loopback_polls = idle_loopback_polls.saturating_add(1);
                    } else {
                        idle_loopback_polls = 0;
                    }
                    if idle_loopback_polls >= MAX_IDLE_LOOPBACK_POLLS {
                        pending.extend(mic_queue.drain(..));
                    } else {
                        mix_into_loopback(&mut pending, &mut loopback_queue, &mut mic_queue);
                    }
                    bound_mic_backlog(&mut mic_queue);
                } else {
                    // System audio is disabled, or its device is still busy:
                    // the microphone is the only clock.
                    pending.extend(mic_queue.drain(..));
                }

                let Some(enc) = encoder.as_ref() else {
                    // Encoder not ready: drop the backlog so a stalled AAC MFT
                    // cannot grow the queues without bound. The retry runs
                    // every second and starts from fresh audio.
                    loopback_queue.clear();
                    mic_queue.clear();
                    pending.clear();
                    std::thread::sleep(std::time::Duration::from_millis(8));
                    continue;
                };

                let epoch_hns = ((qpc_first.unwrap_or(origin) - origin) as i128
                    * HNS_PER_SECOND as i128
                    / freq as i128) as i64;
                // Exact start of output AAC frame `n` (no per-frame rounding
                // accumulation over long sessions).
                let frame_time = |frame: i64| -> i64 {
                    epoch_hns
                        + (frame as i128 * 1024 * HNS_PER_SECOND as i128
                            / SAMPLE_RATE as i128) as i64
                };
                while pending.len() >= FRAME_SAMPLES {
                    pcm.clear();
                    pcm.extend(
                        pending
                            .drain(..FRAME_SAMPLES)
                            .map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i16),
                    );

                    let fed = fed_frames;
                    fed_frames += 1;

                    let gen = generation.load(Ordering::Acquire);
                    let ring_ref = &ring;
                    // The MFT can emit zero payloads (priming) or more than one
                    // (catch-up) for a single input frame. Number the output
                    // frames ourselves, starting no earlier than the input's
                    // own position: that keeps PTS strictly increasing (no
                    // duplicate stamps, which make players stutter) while a
                    // dropped input frame still leaves a real gap in the
                    // timeline instead of compressing the audio.
                    let mut next_frame = fed.max(out_frame);
                    if let Err(e) = enc.encode(&pcm, frame_time(next_frame), |payload| {
                        let out_pts = frame_time(next_frame);
                        next_frame += 1;
                        ring_ref.lock().push(EncodedPacket {
                            track: TrackKind::Audio,
                            data: Arc::from(payload.into_boxed_slice()),
                            pts_hns: out_pts,
                            duration_hns: frame_hns,
                            keyframe: true,
                            generation: gen,
                        });
                    }) {
                        log::warn!("[yascreenclipper::audio] AAC encode failed: {e}");
                    }
                    out_frame = next_frame;
                }

                std::thread::sleep(std::time::Duration::from_millis(8));
            }

            if let Some(ep) = loopback.as_ref() {
                let _ = ep.client.Stop();
            }
            if let Some(ep) = microphone.as_ref() {
                let _ = ep.client.Stop();
            }
            let _ = MFShutdown();
            CoUninitialize();
        }
    }
}

// ---------------------------------------------------------------------------
// Non-Windows stub
// ---------------------------------------------------------------------------

#[cfg(any(not(windows), feature = "headless-sim"))]
mod imp {
    use super::*;

    pub(super) fn run(
        _system: bool,
        _mic: bool,
        _game_gain: Arc<AtomicU32>,
        _mic_gain: Arc<AtomicU32>,
        stop: Arc<AtomicBool>,
        ring: Arc<Mutex<RollingRingBuffer>>,
        generation: Arc<AtomicU32>,
        _status: Arc<Mutex<Option<String>>>,
        _qpc_origin: i64,
        _qpc_freq: i64,
    ) {
        // 21.3 ms AAC frames of silence keep the muxer path exercised.
        let frame_hns = 1024 * HNS_PER_SECOND / SAMPLE_RATE as i64;
        let mut pts = 0i64;
        while !stop.load(Ordering::Acquire) {
            ring.lock().push(EncodedPacket {
                track: TrackKind::Audio,
                data: Arc::from(vec![0u8; 384].into_boxed_slice()),
                pts_hns: pts,
                duration_hns: frame_hns,
                keyframe: true,
                generation: generation.load(Ordering::Acquire),
            });
            pts += frame_hns;
            std::thread::sleep(std::time::Duration::from_millis(21));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine_stereo(frames: usize, rate: u32, freq: f32) -> Vec<f32> {
        let mut v = Vec::with_capacity(frames * 2);
        for i in 0..frames {
            let s = (i as f32 * freq * std::f32::consts::TAU / rate as f32).sin();
            v.push(s);
            v.push(s);
        }
        v
    }

    #[test]
    fn same_rate_resampling_is_transparent() {
        let input = sine_stereo(4096, SAMPLE_RATE, 440.0);
        let mut rs = LinearResampler::new(SAMPLE_RATE);
        let mut out = Vec::new();
        rs.resample(&input, &mut out);
        assert_eq!(out.len(), input.len());
        for (a, b) in out.iter().zip(input.iter()) {
            assert!((a - b).abs() < 1e-5, "{a} vs {b}");
        }
    }

    #[test]
    fn forty_four_one_to_forty_eight_k_produces_the_right_frame_count() {
        // One second at 44.1 kHz ➜ ~48 k frames,-L/R interleaved, finite.
        let input = sine_stereo(44_100, 44_100, 440.0);
        let mut rs = LinearResampler::new(44_100);
        let mut out = Vec::new();
        rs.resample(&input, &mut out);
        let frames = out.len() / 2;
        assert!(
            (47_900..=48_100).contains(&frames),
            "expected ~48000 frames, got {frames}"
        );
        assert!(out.iter().all(|s| s.is_finite()));
        assert!(out.iter().all(|s| *s >= -1.0 && *s <= 1.0));
    }

    #[test]
    fn split_batches_stay_continuous() {
        // Draining in two polls must not click at the seam: the resampler
        // bridges batches through its `prev` frame.
        let input = sine_stereo(4096, 44_100, 440.0);
        let mut whole = Vec::new();
        LinearResampler::new(44_100).resample(&input, &mut whole);
        let mut split = Vec::new();
        let mut rs = LinearResampler::new(44_100);
        rs.resample(&input[..input.len() / 2], &mut split);
        let seam = split.len();
        rs.resample(&input[input.len() / 2..], &mut split);
        assert!((whole.len() as isize - split.len() as isize).abs() <= 4);
        // No discontinuity where the second batch starts. The two samples
        // straddle ~1 output step, so normal 440 Hz slope accounts for up to
        // ~0.06; anything near 1.0 would be an audible click.
        let l = split[seam - 2];
        let r = split[seam];
        assert!((r - l).abs() < 0.15, "click at seam: {l} -> {r}");
    }

    #[test]
    fn empty_and_tiny_inputs_are_safe() {
        let mut rs = LinearResampler::new(44_100);
        let mut out = vec![9.0];
        rs.resample(&[], &mut out);
        rs.resample(&[0.5], &mut out);
        assert_eq!(out, vec![9.0]);
    }

    #[test]
    fn mic_traffic_never_extends_the_game_audio_timeline() {
        // The regression: a poll with a mic packet but no loopback packet
        // appended the mic samples to `pending`, inserting ~180 samples of
        // digital silence into the game audio several times a second. That was
        // the continuous crackle heard once the microphone was enabled.
        let mut pending = Vec::new();
        let mut loopback = Vec::new();
        let mut mic = vec![0.0; 480]; // one silent mic packet

        mix_into_loopback(&mut pending, &mut loopback, &mut mic);
        assert!(pending.is_empty(), "mic-only poll must not emit samples");
        assert_eq!(mic.len(), 480, "mic data must be held for the next poll");

        // The next loopback poll emits exactly the loopback length and mixes
        // the queued mic into it.
        loopback = vec![0.5; 512];
        mix_into_loopback(&mut pending, &mut loopback, &mut mic);
        assert_eq!(pending.len(), 512);
        assert!(pending.iter().all(|s| (*s - 0.5).abs() < 1e-6));
        assert!(mic.is_empty());
    }

    #[test]
    fn mic_tail_is_held_and_mixed_later_instead_of_appended() {
        let mut pending = Vec::new();
        let mut loopback = vec![0.5; 4];
        let mut mic = vec![0.25; 6];

        mix_into_loopback(&mut pending, &mut loopback, &mut mic);

        assert_eq!(pending, vec![0.75; 4]);
        assert_eq!(mic, vec![0.25; 2], "the tail stays queued for later");
        assert!(loopback.is_empty());
    }

    #[test]
    fn the_pad_is_applied_every_poll_not_only_when_the_mic_arrives() {
        // The earlier regression: the game audio level flipped between 100 %
        // and −3 dB at the polling rate because the pad depended on whether a
        // mic packet happened to land in the same poll.
        let loopback: Vec<f32> = sine_stereo(512, SAMPLE_RATE, 440.0)
            .into_iter()
            .map(|s| s * 0.25)
            .collect();

        let mut queue = Vec::new();
        push_source(&mut queue, &loopback, 1.0, MIX_PAD);
        push_source(&mut queue, &[], 1.0, MIX_PAD);
        push_source(&mut queue, &loopback, 1.0, MIX_PAD);

        assert_eq!(queue.len(), loopback.len() * 2);
        for i in 0..loopback.len() {
            assert!((queue[i] - loopback[i] * MIX_PAD).abs() < 1e-6);
            assert!((queue[loopback.len() + i] - loopback[i] * MIX_PAD).abs() < 1e-6);
        }
    }

    #[test]
    fn mic_backlog_is_bounded_in_whole_stereo_frames() {
        let mut mic = vec![0.0; MAX_MIC_BACKLOG + 1000];
        bound_mic_backlog(&mut mic);
        assert_eq!(mic.len(), MAX_MIC_BACKLOG);
    }

    #[test]
    fn packed_24_bit_pcm_is_decoded_not_silenced() {
        // +0.5, −0.5 and full-scale positive, little-endian packed.
        let bytes = [0x00, 0x00, 0x40, 0x00, 0x00, 0xC0, 0xFF, 0xFF, 0x7F];
        let mut out = Vec::new();
        pcm24_to_f32(&bytes, &mut out);
        assert_eq!(out.len(), 3);
        assert!((out[0] - 0.5).abs() < 1e-6);
        assert!((out[1] + 0.5).abs() < 1e-6);
        assert!((out[2] - 0.999_999_9).abs() < 1e-5);
    }
}
