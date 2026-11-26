//! Speech to text, for transcribe_file and the page's Transcribe tab.
//!
//! One recording at a time, on a thread of its own: ffmpeg decodes it to
//! 16 kHz mono, the audio is cut in parts of about ten minutes at a quiet
//! point, and each part goes to whisper-server on the Pi or to Speaches on
//! the desktop through Whoopy. The language is detected on the first part,
//! then fixed. Each part done is saved in <index dir>/transcripts/<key>.json,
//! so a restart resumes after it and a second call is answered from there.
//! Settings, from docs.env:
//!
//!     DOCS_SPEECH_URL          whisper-server, e.g. http://127.0.0.1:8776; unset, no transcription
//!     DOCS_SPEECH_MODEL        its model's name, shown with each transcript
//!     DOCS_SPEECH_ENGINE       pi (default), demand or always: when the desktop is used
//!     DOCS_SPEECH_REMOTE_URL   Whoopy, e.g. http://127.0.0.1:4000
//!     DOCS_SPEECH_REMOTE_MODEL the Speaches model as Whoopy names it
//!     DOCS_SPEECH_REMOTE_KEY   a Whoopy key allowed that model
//!
//! ffmpeg reads the recording from its stdin, opened as /proc/self/fd/0 so an
//! .m4a with its index at the end can be seeked, and only through the file
//! protocol, the audio demuxers and the audio decoders listed here: a playlist
//! or a concat list cannot make it open anything else.

use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{Arc, Mutex, OnceLock, mpsc},
    time::{Duration, Instant, SystemTime},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::nextcloud::hex;

pub const AUDIO_SUFFIXES: &[&str] = &[
    ".mp3", ".m4a", ".aac", ".wav", ".ogg", ".oga", ".opus", ".flac", ".webm", ".mka", ".amr",
    ".3gp", ".mp4",
];
/// Samples per second, as whisper takes them.
const RATE: usize = 16_000;
/// About ten minutes a part: whisper's memory and a desktop request stay small.
const PART_S: usize = 600;
/// A part ends at the quietest tenth of a second of its last minute.
const SEARCH_S: usize = 60;
const FRAME: usize = RATE / 10;
/// Past this, the rest of a recording is left out, and the transcript says so.
pub const MAX_AUDIO_S: usize = 4 * 3600;
/// On disk; a Nextcloud file is capped by the fetch limit.
pub const MAX_AUDIO_BYTES: i64 = 1 << 30;
/// A transcript not asked for again within this is dropped.
const KEEP_DAYS: u64 = 30;
/// The desktop wakes within about two minutes: Whoopy answers 5xx meanwhile.
const RETRY_EVERY: Duration = Duration::from_secs(10);
const RETRY_FOR: Duration = Duration::from_secs(180);
/// Recordings waiting or running at once: an agent sweeping a folder must
/// not queue days of work, and each queued upload holds its file.
pub const MAX_JOBS: usize = 5;
/// The scratch files sit on the disk the photos share: decoding needs at
/// least this much room above write_file's floor.
const PCM_ROOM: u64 = (MAX_AUDIO_S * RATE * 2) as u64;
/// One part on the Pi: large-v3-turbo may take most of it.
const PART_TIMEOUT: Duration = Duration::from_secs(1800);

const DEMUXERS: &str = "mov,mp3,wav,ogg,flac,matroska,aac,amr,caf,aiff,w64";
const DECODERS: &str = "aac,aac_fixed,aac_latm,alac,amrnb,amrwb,flac,mp2,mp2float,mp3,mp3float,\
    opus,libopus,vorbis,libvorbis,pcm_alaw,pcm_mulaw,pcm_f32le,pcm_s16be,pcm_s16le,pcm_s24le,\
    pcm_s32le,pcm_u8,adpcm_ima_wav";

/// whisper's languages: a desktop answer may name one in full.
const LANGUAGES: &[(&str, &str)] = &[
    ("english", "en"),
    ("chinese", "zh"),
    ("german", "de"),
    ("spanish", "es"),
    ("russian", "ru"),
    ("korean", "ko"),
    ("french", "fr"),
    ("japanese", "ja"),
    ("portuguese", "pt"),
    ("turkish", "tr"),
    ("polish", "pl"),
    ("catalan", "ca"),
    ("dutch", "nl"),
    ("arabic", "ar"),
    ("swedish", "sv"),
    ("italian", "it"),
    ("indonesian", "id"),
    ("hindi", "hi"),
    ("finnish", "fi"),
    ("vietnamese", "vi"),
    ("hebrew", "he"),
    ("ukrainian", "uk"),
    ("greek", "el"),
    ("malay", "ms"),
    ("czech", "cs"),
    ("romanian", "ro"),
    ("danish", "da"),
    ("hungarian", "hu"),
    ("tamil", "ta"),
    ("norwegian", "no"),
    ("thai", "th"),
    ("urdu", "ur"),
    ("croatian", "hr"),
    ("bulgarian", "bg"),
    ("lithuanian", "lt"),
    ("latin", "la"),
    ("maori", "mi"),
    ("malayalam", "ml"),
    ("welsh", "cy"),
    ("slovak", "sk"),
    ("telugu", "te"),
    ("persian", "fa"),
    ("latvian", "lv"),
    ("bengali", "bn"),
    ("serbian", "sr"),
    ("azerbaijani", "az"),
    ("slovenian", "sl"),
    ("kannada", "kn"),
    ("estonian", "et"),
    ("macedonian", "mk"),
    ("breton", "br"),
    ("basque", "eu"),
    ("icelandic", "is"),
    ("armenian", "hy"),
    ("nepali", "ne"),
    ("mongolian", "mn"),
    ("bosnian", "bs"),
    ("kazakh", "kk"),
    ("albanian", "sq"),
    ("swahili", "sw"),
    ("galician", "gl"),
    ("marathi", "mr"),
    ("punjabi", "pa"),
    ("sinhala", "si"),
    ("khmer", "km"),
    ("shona", "sn"),
    ("yoruba", "yo"),
    ("somali", "so"),
    ("afrikaans", "af"),
    ("occitan", "oc"),
    ("georgian", "ka"),
    ("belarusian", "be"),
    ("tajik", "tg"),
    ("sindhi", "sd"),
    ("gujarati", "gu"),
    ("amharic", "am"),
    ("yiddish", "yi"),
    ("lao", "lo"),
    ("uzbek", "uz"),
    ("faroese", "fo"),
    ("haitian creole", "ht"),
    ("pashto", "ps"),
    ("turkmen", "tk"),
    ("nynorsk", "nn"),
    ("maltese", "mt"),
    ("sanskrit", "sa"),
    ("luxembourgish", "lb"),
    ("myanmar", "my"),
    ("tibetan", "bo"),
    ("tagalog", "tl"),
    ("malagasy", "mg"),
    ("assamese", "as"),
    ("tatar", "tt"),
    ("hawaiian", "haw"),
    ("lingala", "ln"),
    ("hausa", "ha"),
    ("bashkir", "ba"),
    ("javanese", "jw"),
    ("sundanese", "su"),
    ("cantonese", "yue"),
];

/// The language code of an answer's "language": a code, or a name in full.
pub fn language_code(given: &str) -> Option<&'static str> {
    let given = given.trim().to_lowercase();
    LANGUAGES
        .iter()
        .find(|(name, code)| *name == given || *code == given)
        .map(|(_, code)| *code)
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Engine {
    Pi,
    Desktop,
}

struct Remote {
    url: String,
    model: String,
    key: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Segment {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// What the cache holds of one recording, done or in part.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Transcript {
    /// Its tool path, or "upload: <name>".
    pub shown: String,
    pub done: bool,
    /// Why the last attempt failed; empty when it did not.
    pub error: String,
    /// Seconds of audio, up to MAX_AUDIO_S.
    pub duration: f64,
    /// Seconds transcribed so far: always the end of a part.
    pub done_until: f64,
    pub language: String,
    /// Where it was transcribed, as said to the reader.
    pub engines: Vec<String>,
    pub note: String,
    pub segments: Vec<Segment>,
}

/// Where a recording stands.
pub enum Look {
    Done(Transcript),
    /// Queued behind `ahead` others, or running.
    Busy {
        running: bool,
        ahead: usize,
        done: f64,
        total: f64,
        left: Option<Duration>,
    },
    /// Failed at `at` seconds; tried again only when asked.
    Failed {
        error: String,
        at: f64,
    },
    /// Begun, then stopped by a restart: carried on when asked again.
    Stopped {
        done: f64,
        total: f64,
    },
    Unknown,
}

/// Opens the recording when its turn comes, on the speech thread.
pub type Opener = Box<dyn FnOnce() -> Result<File, String> + Send>;

pub struct Job {
    pub key: String,
    pub shown: String,
    pub engine: Engine,
    /// A language code, or empty to detect it.
    pub language: String,
    pub open: Opener,
}

#[derive(Clone)]
struct Running {
    done: f64,
    total: f64,
    /// Where this run started, to say how long the rest takes.
    from: f64,
    since: Instant,
}

pub struct Speech {
    pi_url: String,
    pi_model: String,
    remote: Option<Remote>,
    /// The desktop unless the call asks for the Pi.
    always: bool,
    dir: PathBuf,
    /// Keys queued, in order; the first is running once started.
    jobs: Mutex<Vec<(String, Option<Running>)>>,
    queue: OnceLock<Mutex<mpsc::Sender<Job>>>,
}

/// A key from the parts that name a recording and how it is transcribed:
/// its user, path, size and date, and the engine's name with its model.
pub fn key(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p.as_bytes());
        h.update([0]);
    }
    hex(&h.finalize())
}

/// A file in the unit's private /tmp, gone from it at once: nothing is left
/// behind whatever happens next.
pub fn scratch() -> Result<File, String> {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path =
        std::env::temp_dir().join(format!(".cheruby-docs-speech-{}-{n}", std::process::id()));
    let f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| format!("a scratch file: {}", crate::folders::io_message(&e)))?;
    let _ = std::fs::remove_file(&path);
    Ok(f)
}

/// Refused when a file there would leave the disk less than write_file's
/// floor plus `more` bytes.
pub fn room(f: &File, more: u64) -> Result<(), String> {
    let free = nix::sys::statvfs::fstatvfs(f).map_err(|e| e.to_string())?;
    if (free.blocks_available() as u64) * (free.fragment_size() as u64)
        < more + crate::folders::MIN_FREE_BYTES
    {
        return Err("the disk is nearly full; nothing is transcribed.".into());
    }
    Ok(())
}

/// A reader copied into a scratch file, at its start, so ffmpeg can seek it.
pub fn spill(mut from: impl Read) -> Result<File, String> {
    let mut f = scratch()?;
    room(&f, crate::tools::MAX_FETCH_BYTES)?;
    std::io::copy(&mut from, &mut f)
        .and_then(|_| f.seek(SeekFrom::Start(0)))
        .map_err(|e| crate::folders::io_message(&e))?;
    Ok(f)
}

/// The start of the quietest frame of a window of samples, plus half a frame.
fn quietest(window: &[i16]) -> usize {
    let mut best = (u64::MAX, 0);
    for (i, frame) in window.chunks(FRAME).enumerate() {
        if frame.len() < FRAME {
            break;
        }
        let energy: u64 = frame.iter().map(|&s| (s as i64 * s as i64) as u64).sum();
        if energy < best.0 {
            best = (energy, i * FRAME);
        }
    }
    best.1 + FRAME / 2
}

/// Where the parts of `total` samples start and end: [0, c1, c2, ..., total].
/// window(from, to) gives the samples between two points.
fn cuts(total: usize, mut window: impl FnMut(usize, usize) -> Vec<i16>) -> Vec<usize> {
    let mut cuts = vec![0];
    let mut start = 0;
    while total - start > PART_S * RATE {
        let mark = start + PART_S * RATE;
        let from = mark - SEARCH_S * RATE;
        start = from + quietest(&window(from, mark));
        cuts.push(start);
    }
    cuts.push(total);
    cuts
}

/// A WAV of 16-bit mono samples at RATE.
fn wav(samples: &[u8]) -> Vec<u8> {
    let n = samples.len() as u32;
    let mut out = Vec::with_capacity(44 + samples.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + n).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(RATE as u32).to_le_bytes());
    out.extend_from_slice(&(RATE as u32 * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&n.to_le_bytes());
    out.extend_from_slice(samples);
    out
}

/// A multipart/form-data body: the fields, then the audio as "file".
fn multipart(fields: &[(&str, &str)], audio: &[u8]) -> (String, Vec<u8>) {
    let boundary = format!(
        "cheruby{}",
        &key(&[&format!("{:?}", SystemTime::now())])[..32]
    );
    let mut body = Vec::with_capacity(audio.len() + 1024);
    for (name, value) in fields {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"part.wav\"\r\nContent-Type: audio/wav\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(audio);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

/// h:mm:ss, or m:ss under an hour.
pub fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// The text: a line per segment with its time, or paragraphs split at pauses.
/// whisper starts each segment with a space, except one it began inside a
/// word: that one is joined to the previous segment.
pub fn render(t: &Transcript, timestamps: bool) -> String {
    let mut out = String::new();
    let mut last_end = None;
    for s in &t.segments {
        let text = s.text.trim();
        if text.is_empty() {
            continue;
        }
        if !out.is_empty() && !s.text.starts_with(char::is_whitespace) {
            if timestamps {
                out.pop();
                out += text;
                out.push('\n');
            } else {
                out += text;
                last_end = Some(s.end);
            }
            continue;
        }
        if timestamps {
            out += &format!("[{}] {text}\n", clock(s.start));
            continue;
        }
        match last_end {
            None => {}
            Some(end) if s.start - end >= 2.0 => out += "\n\n",
            Some(_) => out.push(' '),
        }
        out += text;
        last_end = Some(s.end);
    }
    out.trim_end().to_string()
}

/// A whisper or OpenAI verbose_json answer: its segments and language.
fn parse_answer(data: &Value) -> Result<(Vec<Segment>, String), String> {
    let segments = data
        .get("segments")
        .and_then(Value::as_array)
        .ok_or("the answer has no segments.")?
        .iter()
        .map(|s| Segment {
            start: s.get("start").and_then(Value::as_f64).unwrap_or(0.0),
            end: s.get("end").and_then(Value::as_f64).unwrap_or(0.0),
            text: s
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        })
        .collect();
    let language = data
        .get("language")
        .and_then(Value::as_str)
        .and_then(language_code)
        .unwrap_or("")
        .to_string();
    Ok((segments, language))
}

impl Speech {
    /// None when DOCS_SPEECH_URL is unset: no transcription is offered.
    pub fn from_env(index_dir: &std::path::Path) -> Result<Option<Speech>, String> {
        let var = |k: &str| std::env::var(k).unwrap_or_default();
        let pi_url = var("DOCS_SPEECH_URL");
        if pi_url.is_empty() {
            return Ok(None);
        }
        let engine = var("DOCS_SPEECH_ENGINE");
        let remote = Some(Remote {
            url: var("DOCS_SPEECH_REMOTE_URL"),
            model: var("DOCS_SPEECH_REMOTE_MODEL"),
            key: var("DOCS_SPEECH_REMOTE_KEY"),
        })
        .filter(|r| !r.url.is_empty() && !r.model.is_empty() && !r.key.is_empty());
        let always = match engine.as_str() {
            "" | "pi" => false,
            "demand" => false,
            "always" => true,
            other => {
                return Err(format!(
                    "DOCS_SPEECH_ENGINE: {} is not pi, demand or always.",
                    crate::folders::py_repr(other)
                ));
            }
        };
        let remote = remote.filter(|_| matches!(engine.as_str(), "demand" | "always"));
        Ok(Some(Speech {
            pi_url,
            pi_model: var("DOCS_SPEECH_MODEL"),
            always: always && remote.is_some(),
            remote,
            dir: index_dir.join("transcripts"),
            jobs: Mutex::default(),
            queue: OnceLock::new(),
        }))
    }

    pub fn pi_url(&self) -> &str {
        &self.pi_url
    }

    /// The engines a call may ask for, the default first.
    pub fn engines(&self) -> Vec<&'static str> {
        match (&self.remote, self.always) {
            (None, _) => vec!["pi"],
            (Some(_), true) => vec!["desktop", "pi"],
            (Some(_), false) => vec!["pi", "desktop"],
        }
    }

    /// The engine a call asks for: "pi", "desktop", or empty for the default.
    pub fn engine(&self, asked: &str) -> Result<Engine, String> {
        match asked {
            "" if self.always => Ok(Engine::Desktop),
            "" | "pi" => Ok(Engine::Pi),
            "desktop" if self.remote.is_some() => Ok(Engine::Desktop),
            "desktop" => Err("the desktop is not set up for transcription; use the Pi.".into()),
            other => Err(format!(
                "engine: {} is not pi or desktop.",
                crate::folders::py_repr(other)
            )),
        }
    }

    fn file(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.json"))
    }

    fn load(&self, key: &str) -> Option<Transcript> {
        let text = std::fs::read_to_string(self.file(key)).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Written beside itself then renamed, so a reader never sees half of it.
    fn save(&self, key: &str, t: &Transcript) -> Result<(), String> {
        let _ = std::fs::create_dir_all(&self.dir);
        let tmp = self.dir.join(format!(".{key}.tmp"));
        let text = serde_json::to_string(t).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, text)
            .and_then(|()| std::fs::rename(&tmp, self.file(key)))
            .map_err(|e| {
                format!(
                    "the transcript could not be saved: {}",
                    crate::folders::io_message(&e)
                )
            })
    }

    /// Where a recording stands, from the queue and the cache.
    pub fn look(&self, key: &str) -> Look {
        let jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(at) = jobs.iter().position(|(k, _)| k == key) {
            let running = jobs[at].1.clone();
            let saved = self.load(key).unwrap_or_default();
            return match running {
                Some(r) => {
                    let rate = (r.done - r.from) / r.since.elapsed().as_secs_f64().max(1.0);
                    let left =
                        (rate > 0.0).then(|| Duration::from_secs_f64((r.total - r.done) / rate));
                    Look::Busy {
                        running: true,
                        ahead: at,
                        done: r.done,
                        total: r.total,
                        left,
                    }
                }
                None => Look::Busy {
                    running: false,
                    ahead: at,
                    done: saved.done_until,
                    total: saved.duration,
                    left: None,
                },
            };
        }
        drop(jobs);
        match self.load(key) {
            Some(t) if t.done => {
                // Kept KEEP_DAYS from its last read, not its making.
                if let Ok(f) = File::options().append(true).open(self.file(key)) {
                    let _ = f.set_modified(SystemTime::now());
                }
                Look::Done(t)
            }
            Some(t) if !t.error.is_empty() => Look::Failed {
                error: t.error,
                at: t.done_until,
            },
            Some(t) => Look::Stopped {
                done: t.done_until,
                total: t.duration,
            },
            None => Look::Unknown,
        }
    }

    /// Whether a new recording would be refused: MAX_JOBS wait or run.
    pub fn full(&self) -> bool {
        self.jobs.lock().unwrap_or_else(|e| e.into_inner()).len() >= MAX_JOBS
    }

    /// Queues a recording, unless it is queued already. The speech thread
    /// starts with the first one.
    pub fn start(self: &Arc<Self>, job: Job) -> Result<(), String> {
        let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        if jobs.iter().any(|(k, _)| *k == job.key) {
            return Ok(());
        }
        if jobs.len() >= MAX_JOBS {
            return Err(format!(
                "{MAX_JOBS} recordings are being transcribed or wait already; ask again once one is done."
            ));
        }
        let sender = self.queue.get_or_init(|| {
            let (tx, rx) = mpsc::channel::<Job>();
            let speech = Arc::clone(self);
            std::thread::spawn(move || speech.work(rx));
            Mutex::new(tx)
        });
        jobs.push((job.key.clone(), None));
        sender
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .send(job)
            .map_err(|_| "the transcription thread has stopped.".to_string())
    }

    /// The speech thread: each job in turn, for as long as the server runs.
    /// Its HTTP client is made here, outside the runtime.
    fn work(&self, rx: mpsc::Receiver<Job>) {
        let http = reqwest::blocking::Client::builder()
            .timeout(PART_TIMEOUT)
            .no_proxy()
            .build()
            .expect("an HTTP client");
        for job in rx {
            let key = job.key.clone();
            self.forget_old();
            if let Err(e) = self.transcribe(&http, job) {
                eprintln!("transcription failed: {e}");
                let mut t = self.load(&key).unwrap_or_default();
                t.error = e;
                let _ = self.save(&key, &t);
            }
            let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
            jobs.retain(|(k, _)| *k != key);
        }
    }

    /// Drops transcripts not touched for KEEP_DAYS.
    fn forget_old(&self) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        let limit = Duration::from_secs(KEEP_DAYS * 86_400);
        for entry in entries.flatten() {
            let old = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > limit);
            if old {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    fn progress(&self, key: &str, done: f64, total: f64, from: Option<f64>) {
        let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, r)) = jobs.iter_mut().find(|(k, _)| k == key) {
            match r {
                Some(r) => r.done = done,
                None => {
                    *r = Some(Running {
                        done,
                        total,
                        from: from.unwrap_or(done),
                        since: Instant::now(),
                    })
                }
            }
        }
    }

    fn transcribe(&self, http: &reqwest::blocking::Client, job: Job) -> Result<(), String> {
        let mut t = self.load(&job.key).unwrap_or_default();
        if t.done {
            return Ok(());
        }
        t.shown = job.shown.clone();
        t.error.clear();
        let input = (job.open)()?;
        let mut pcm = decode(input)?;
        let bytes = pcm.metadata().map_err(|e| e.to_string())?.len() as usize;
        let total = bytes / 2;
        if total < RATE / 2 {
            return Err(format!("{}: no audio in it.", job.shown));
        }
        t.duration = total as f64 / RATE as f64;
        if total >= MAX_AUDIO_S * RATE {
            t.note = format!(
                "Only its first {} hours are transcribed.",
                MAX_AUDIO_S / 3600
            );
        }
        let mut read = |from: usize, to: usize| -> std::io::Result<Vec<u8>> {
            let mut buf = vec![0u8; (to - from) * 2];
            pcm.seek(SeekFrom::Start(from as u64 * 2))?;
            pcm.read_exact(&mut buf)?;
            Ok(buf)
        };
        let mut failed = None;
        let cuts = cuts(total, |from, to| {
            read(from, to)
                .map(|b| {
                    b.as_chunks::<2>()
                        .0
                        .iter()
                        .map(|c| i16::from_le_bytes(*c))
                        .collect()
                })
                .unwrap_or_else(|e| {
                    failed = Some(e);
                    Vec::new()
                })
        });
        if let Some(e) = failed {
            return Err(crate::folders::io_message(&e));
        }
        if t.language.is_empty() {
            t.language = job.language.clone();
        }
        let mut engine = job.engine;
        self.progress(&job.key, t.done_until, t.duration, None);
        for part in cuts.windows(2) {
            let (from, to) = (part[0], part[1]);
            let start_s = from as f64 / RATE as f64;
            if (to as f64 / RATE as f64) <= t.done_until + 0.001 {
                continue;
            }
            let audio = wav(&read(from, to).map_err(|e| crate::folders::io_message(&e))?);
            let answer = match engine {
                Engine::Desktop => match self.desktop(http, &audio, &t.language) {
                    Ok(a) => Ok((a, self.name(Engine::Desktop))),
                    Err(e) => {
                        eprintln!("transcription on the desktop failed: {e}");
                        t.note = format!(
                            "{} The desktop failed ({e}); the rest, from {}, was done on the Pi.",
                            t.note,
                            clock(start_s)
                        )
                        .trim()
                        .to_string();
                        engine = Engine::Pi;
                        self.pi(http, &audio, &t.language)
                            .map(|a| (a, self.name(Engine::Pi)))
                    }
                },
                Engine::Pi => self
                    .pi(http, &audio, &t.language)
                    .map(|a| (a, self.name(Engine::Pi))),
            };
            let ((segments, language), name) = answer?;
            if t.language.is_empty() {
                t.language = language;
            }
            if !t.engines.contains(&name) {
                t.engines.push(name);
            }
            t.segments.extend(segments.into_iter().map(|s| Segment {
                start: s.start + start_s,
                end: s.end + start_s,
                text: s.text,
            }));
            t.done_until = to as f64 / RATE as f64;
            self.save(&job.key, &t)?;
            self.progress(&job.key, t.done_until, t.duration, None);
        }
        t.done = true;
        self.save(&job.key, &t)
    }

    /// Where an engine transcribes, with its model: said to the reader, and
    /// part of the cache key, so another model gives another transcript.
    pub fn name(&self, engine: Engine) -> String {
        match engine {
            Engine::Pi if self.pi_model.is_empty() => "the Pi".into(),
            Engine::Pi => format!("the Pi ({})", self.pi_model),
            Engine::Desktop => {
                let model = self.remote.as_ref().map(|r| r.model.as_str()).unwrap_or("");
                format!("the desktop ({model})")
            }
        }
    }

    /// One part on whisper-server, which detects the language when told "auto".
    fn pi(
        &self,
        http: &reqwest::blocking::Client,
        audio: &[u8],
        language: &str,
    ) -> Result<(Vec<Segment>, String), String> {
        let language = if language.is_empty() {
            "auto"
        } else {
            language
        };
        let (kind, body) = multipart(
            &[
                ("response_format", "verbose_json"),
                ("language", language),
                ("no_language_probabilities", "true"),
                ("temperature", "0.0"),
            ],
            audio,
        );
        let resp = http
            .post(format!("{}/inference", self.pi_url.trim_end_matches('/')))
            .header("content-type", kind)
            .body(body)
            .send()
            .map_err(|e| format!("the speech server cannot be reached: {}", short(&e)))?;
        let status = resp.status();
        let data: Value = resp.json().unwrap_or(Value::Null);
        if !status.is_success() {
            return Err(format!("the speech server answered {}.", status.as_u16()));
        }
        parse_answer(&data)
    }

    /// One part on the desktop through Whoopy, retried while the desktop wakes.
    fn desktop(
        &self,
        http: &reqwest::blocking::Client,
        audio: &[u8],
        language: &str,
    ) -> Result<(Vec<Segment>, String), String> {
        let remote = self.remote.as_ref().ok_or("the desktop is not set up.")?;
        let mut fields = vec![
            ("model", remote.model.as_str()),
            ("response_format", "verbose_json"),
            ("timestamp_granularities[]", "segment"),
        ];
        if !language.is_empty() {
            fields.push(("language", language));
        }
        let url = format!(
            "{}/v1/audio/transcriptions",
            remote.url.trim_end_matches('/')
        );
        let until = Instant::now() + RETRY_FOR;
        loop {
            let (kind, body) = multipart(&fields, audio);
            let sent = http
                .post(&url)
                .bearer_auth(&remote.key)
                .header("content-type", kind)
                .body(body)
                .send();
            let again = match sent {
                Ok(resp) if resp.status().is_success() => {
                    return parse_answer(&resp.json().unwrap_or(Value::Null));
                }
                Ok(resp) => {
                    let code = resp.status().as_u16();
                    if !matches!(code, 500 | 502..=504) {
                        return Err(format!("Whoopy answered {code}"));
                    }
                    format!("Whoopy answered {code}")
                }
                Err(e) if e.is_connect() => format!("Whoopy cannot be reached: {}", short(&e)),
                Err(e) => return Err(format!("Whoopy: {}", short(&e))),
            };
            if Instant::now() + RETRY_EVERY > until {
                return Err(again);
            }
            std::thread::sleep(RETRY_EVERY);
        }
    }
}

/// An HTTP error's innermost cause, without the URL.
fn short(e: &reqwest::Error) -> String {
    let mut text = e.to_string();
    let mut source = std::error::Error::source(e);
    while let Some(s) = source {
        text = s.to_string();
        source = s.source();
    }
    text
}

/// The recording as 16 kHz mono samples, in a scratch file, by ffmpeg
/// reading its stdin through the whitelists above.
fn decode(input: File) -> Result<File, String> {
    let out = scratch()?;
    room(&out, PCM_ROOM)?;
    // Not the tokens and keys of the server's environment.
    let output = Command::new("ffmpeg")
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .args(["-hide_banner", "-loglevel", "error", "-nostdin"])
        .args(["-protocol_whitelist", "file", "-format_whitelist", DEMUXERS])
        .args(["-codec_whitelist", DECODERS])
        .args([
            "-i",
            "/proc/self/fd/0",
            "-map",
            "0:a:0",
            "-vn",
            "-sn",
            "-dn",
        ])
        .args(["-t", &MAX_AUDIO_S.to_string()])
        .args([
            "-ac",
            "1",
            "-ar",
            &RATE.to_string(),
            "-f",
            "s16le",
            "pipe:1",
        ])
        .stdin(Stdio::from(input))
        .stdout(Stdio::from(out.try_clone().map_err(|e| e.to_string())?))
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("ffmpeg cannot be run: {}", crate::folders::io_message(&e)))?;
    if !output.status.success() {
        let said = String::from_utf8_lossy(&output.stderr);
        // ffmpeg names its input /proc/self/fd/0, never a real path.
        let said: String = said
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("not a recording ffmpeg can read")
            .chars()
            .take(200)
            .collect();
        return Err(format!("the audio could not be decoded: {}", said.trim()));
    }
    let mut out = out;
    out.flush().map_err(|e| e.to_string())?;
    Ok(out)
}

/// The answers the page shows for a key: where it stands and, once done, its text.
pub fn look_json(look: Look) -> Value {
    use serde_json::json;
    match look {
        Look::Done(t) => json!({
            "state": "done",
            "shown": t.shown,
            "duration": t.duration,
            "language": t.language,
            "engines": t.engines,
            "note": t.note,
            "text": render(&t, true),
        }),
        Look::Busy {
            running,
            ahead,
            done,
            total,
            left,
        } => json!({
            "state": if running { "running" } else { "queued" },
            "ahead": ahead,
            "done": done,
            "total": total,
            "left": left.map(|d| d.as_secs()),
        }),
        Look::Failed { error, at } => json!({"state": "failed", "error": error, "done": at}),
        Look::Stopped { done, total } => json!({"state": "stopped", "done": done, "total": total}),
        Look::Unknown => json!({"state": "unknown"}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_part_ends_in_the_quietest_frame_of_its_last_minute() {
        // 25 minutes of noise with two pauses, at 9:30 and 19:10.
        let total = 25 * 60 * RATE;
        let quiet = [570 * RATE, 1150 * RATE];
        let sample = |i: usize| -> i16 {
            if quiet.iter().any(|&q| i >= q && i < q + FRAME * 3) {
                0
            } else {
                ((i * 7919) % 2000) as i16 - 1000
            }
        };
        let got = cuts(total, |a, b| (a..b).map(sample).collect());
        assert_eq!(got.len(), 4);
        assert!(
            got[1] >= quiet[0] && got[1] < quiet[0] + FRAME * 3,
            "{got:?}"
        );
        assert!(
            got[2] >= quiet[1] && got[2] < quiet[1] + FRAME * 3,
            "{got:?}"
        );
        assert_eq!(got[3], total);
    }

    #[test]
    fn a_short_recording_is_one_part() {
        assert_eq!(
            cuts(PART_S * RATE, |_, _| unreachable!()),
            vec![0, PART_S * RATE]
        );
    }

    #[test]
    fn languages_by_name_or_code() {
        assert_eq!(language_code("French"), Some("fr"));
        assert_eq!(language_code("fr"), Some("fr"));
        assert_eq!(language_code("klingon"), None);
    }

    #[test]
    fn times_or_paragraphs() {
        let t = Transcript {
            segments: vec![
                Segment {
                    start: 0.0,
                    end: 2.0,
                    text: " Bonjour.".into(),
                },
                Segment {
                    start: 2.5,
                    end: 4.0,
                    text: " Ça va ?".into(),
                },
                Segment {
                    start: 3725.0,
                    end: 3726.0,
                    text: " Au revoir.".into(),
                },
            ],
            ..Default::default()
        };
        assert_eq!(
            render(&t, true),
            "[0:00] Bonjour.\n[0:02] Ça va ?\n[1:02:05] Au revoir."
        );
        assert_eq!(render(&t, false), "Bonjour. Ça va ?\n\nAu revoir.");
    }

    #[test]
    fn a_word_cut_between_segments_is_joined() {
        let seg = |start: f64, text: &str| Segment {
            start,
            end: start + 1.0,
            text: text.into(),
        };
        let t = Transcript {
            segments: vec![
                seg(0.0, " Payer les fact"),
                seg(1.0, "ures, activer une carte."),
                seg(2.0, " Merci."),
            ],
            ..Default::default()
        };
        assert_eq!(
            render(&t, true),
            "[0:00] Payer les factures, activer une carte.\n[0:02] Merci."
        );
        assert_eq!(
            render(&t, false),
            "Payer les factures, activer une carte. Merci."
        );
    }

    #[test]
    fn a_wav_says_its_length() {
        let w = wav(&[0u8; 64]);
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(u32::from_le_bytes([w[40], w[41], w[42], w[43]]), 64);
        assert_eq!(w.len(), 44 + 64);
    }
}
