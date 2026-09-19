//! Converts public running and inertial datasets into the small tables the
//! real-data tests read.
//!
//! The tests compare the simulator's inertial signature against real recordings —
//! step frequency, the amplitude of the fundamental and the ratio of the second
//! harmonic to it — because those three numbers are what the design says must be
//! calibrated from real IMU data and cannot be derived from a specification.
//!
//! ```text
//! cargo run -p ourealis-core --example fetch_real_data
//! ```
//!
//! The tool walks the archives in [`SOURCES`], takes the running or jogging
//! portion of each, and writes one table per source into `data/real/`:
//!
//! ```text
//! data/real/<source>.csv             subject,ax,ay,az   (one line per sample)
//! data/real/<source>.meta            url, activity, channel, subject and sample counts
//! data/real/<source>_<channel>.csv   a further channel of the same collection
//! ```
//!
//! The archives live in `data/raw/<file>` and are never committed — see
//! `.gitignore`. A source whose archive is missing is **skipped with its URL
//! printed**, so the tool is useful with whichever datasets are on hand and the
//! tests grow automatically as more arrive.
//!
//! ## What the sources are for
//!
//! No single collection answers every calibration question:
//!
//! | Source | Contributes |
//! |---|---|
//! | WISDM (UCI 507) | jogging cadence and the gyroscope's step signature, both channels at 20 Hz |
//! | MotionSense (GitHub) | 50 Hz phone inertial data, so the harmonics are resolvable above the third |
//! | Daily and Sports Activities (UCI 256) | treadmill running at two speeds: the only source here that varies speed, which a cadence–speed fit needs |
//! | HAR (UCI 240) | walking at 50 Hz, for the walking mode the design only reserves |
//!
//! Sampling rate decides how far up the harmonic series a recording can be
//! measured: at 20 Hz the third harmonic of a 3 Hz step sits a tenth of a hertz
//! below Nyquist, so its amplitude is not trustworthy. Sensor placement decides
//! what the *shape* of the signal means: a phone in a pocket, a unit on the torso
//! and a watch on a wrist see the same cadence with different harmonics. Activity
//! labelling decides whether the rows can be selected at all without guessing from
//! the signal.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Running or jogging samples per subject: one `[x, y, z]` triple each.
type Subjects = BTreeMap<u32, Vec<[f64; 3]>>;

/// A collection the tool knows how to read.
struct Source {
    /// Table name written under `data/real/`.
    key: &'static str,
    /// Archive file name under `data/raw/`.
    archive: &'static str,
    /// Where the archive comes from.
    url: &'static str,
    /// Which sensor the rows come from, in preference order.
    channels: &'static [Channel],
    /// Layout of the archive.
    layout: Layout,
    /// What the rows are, for the table's metadata.
    activity: &'static str,
    /// Sampling rate of the rows, Hz.
    rate_hz: f64,
}

/// Inertial channel of a recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Channel {
    Accel,
    Gyro,
}

impl Channel {
    fn name(self) -> &'static str {
        match self {
            Channel::Accel => "accel",
            Channel::Gyro => "gyro",
        }
    }
}

/// How an archive is laid out, and how a row is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Layout {
    /// One file per channel holding every subject, rows of
    /// `subject,activity,timestamp,x,y,z` with a trailing semicolon.
    WisimRows,
    /// One directory per activity and trial, one file per subject, a header row
    /// then `userAcceleration.x, …, rotationRate.x, …`, comma separated.
    MotionSense,
    /// One directory per subject containing one directory per activity, each
    /// holding one file per body-worn unit: a numeric matrix, no header.
    DailySports,
    /// `…/Inertial Signals/body_acc_{x,y,z}_{train,test}.txt`: one window per
    /// line, the three axes in three files.
    HarSignals,
}

/// Number of samples kept per source, so the tables stay small.
const MAX_SAMPLES_PER_SOURCE: usize = 400_000;

/// Number of subjects kept per source.
const MAX_SUBJECTS: usize = 20;

const SOURCES: &[Source] = &[
    Source {
        key: "wisdm2",
        archive: "wisdm2.zip",
        url: "https://archive.ics.uci.edu/static/public/507/wisdm+smartphone+and+smartwatch+activity+and+biometrics+dataset.zip",
        channels: &[Channel::Accel, Channel::Gyro],
        layout: Layout::WisimRows,
        activity: "jogging",
        rate_hz: 20.0,
    },
    Source {
        key: "motionsense",
        archive: "motionsense.zip",
        url: "https://codeload.github.com/mmalekzadeh/motion-sense/zip/refs/heads/master",
        channels: &[Channel::Accel, Channel::Gyro],
        layout: Layout::MotionSense,
        activity: "jogging",
        rate_hz: 50.0,
    },
    Source {
        key: "dasa",
        archive: "dasa.zip",
        url: "https://archive.ics.uci.edu/static/public/256/daily+and+sports+activities.zip",
        channels: &[Channel::Accel],
        // The two treadmill-running activities. The published activity list puts
        // them at a09 (8 km/h) and a10 (12 km/h); the tool prints every activity
        // it sees, so a different ordering is visible rather than silent.
        layout: Layout::DailySports,
        activity: "running (treadmill)",
        rate_hz: 25.0,
    },
    Source {
        key: "har",
        archive: "har.zip",
        url: "https://archive.ics.uci.edu/static/public/240/human+activity+recognition+using+smartphones.zip",
        channels: &[Channel::Accel],
        // Walking only, and only through the raw inertial signals: the collection's
        // headline files are pre-extracted features, which cannot calibrate a
        // spectrum.
        layout: Layout::HarSignals,
        activity: "walking",
        rate_hz: 50.0,
    },
];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = workspace_root();
    let raw = root.join("data/raw");
    let out_dir = root.join("data/real");
    fs::create_dir_all(&raw)?;
    fs::create_dir_all(&out_dir)?;

    let mut written = 0usize;
    let mut missing = Vec::new();
    for source in SOURCES {
        let archive = raw.join(source.archive);
        if !archive.exists() {
            missing.push(source);
            continue;
        }
        match convert(source, &archive, &out_dir) {
            Ok(samples) => {
                println!("{}: wrote {samples} sample(s)", source.key);
                written += 1;
            }
            Err(error) => println!("{}: skipped ({error})", source.key),
        }
    }

    if !missing.is_empty() {
        println!();
        println!(
            "{} archive(s) not present in {}; download them there to widen the tests:",
            missing.len(),
            raw.display()
        );
        for source in &missing {
            println!("  {}  ->  data/raw/{}", source.url, source.archive);
        }
    }
    if written == 0 {
        return Err("no archive was converted".into());
    }
    Ok(())
}

/// Root of the workspace, so the tool works from any directory.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Converts one archive into `data/real/<key>.csv` plus its metadata.
fn convert(
    source: &Source,
    archive: &Path,
    out_dir: &Path,
) -> Result<usize, Box<dyn std::error::Error>> {
    let bytes = fs::read(archive)?;
    if bytes.len() < 4 || &bytes[..4] != b"PK\x03\x04" {
        return Err("not a zip archive, or not a complete one".into());
    }
    let entries = collect_entries(&bytes, 0)?;
    println!(
        "{}: {} file(s) visible in the archive tree",
        source.key,
        entries.len()
    );

    // Every channel the archive yields becomes its own table. A source that
    // carries both an accelerometer and a gyroscope answers two different
    // calibration questions — the step harmonics live on the accelerometer, the
    // step signature on the gyroscope — and stopping at the first channel that
    // parses would leave the second question with no reference data at all.
    let mut last_error = String::from("no recording was found");
    let mut total = 0usize;
    for channel in source.channels {
        match parse(source, &entries, *channel) {
            Ok(rows) if !rows.is_empty() => {
                total += rows.values().map(|samples| samples.len()).sum::<usize>();
                write_table(source, out_dir, *channel, &rows)?;
            }
            Ok(_) => last_error = format!("no {} rows were found", channel.name()),
            Err(error) => last_error = error.to_string(),
        }
    }
    if total == 0 {
        return Err(last_error.into());
    }
    Ok(total)
}

/// Writes the table and its metadata.
///
/// The first channel a source declares keeps the plain `<source>.csv` name; any
/// further channel is written as `<source>_<channel>.csv`, so the primary table
/// keeps the name the documentation uses.
fn write_table(
    source: &Source,
    out_dir: &Path,
    channel: Channel,
    rows: &Subjects,
) -> Result<(), Box<dyn std::error::Error>> {
    let name = if source.channels.first() == Some(&channel) {
        source.key.to_string()
    } else {
        format!("{}_{}", source.key, channel.name())
    };
    let table = out_dir.join(format!("{name}.csv"));
    let mut file = fs::File::create(&table)?;
    writeln!(file, "subject,ax,ay,az")?;
    let mut written = 0usize;
    for (subject, samples) in rows {
        for sample in samples {
            writeln!(
                file,
                "{subject},{:.5},{:.5},{:.5}",
                sample[0], sample[1], sample[2]
            )?;
            written += 1;
        }
    }
    fs::write(
        out_dir.join(format!("{name}.meta")),
        format!(
            "source = {}\n\
             url = {}\n\
             activity = {}\n\
             channel = {}\n\
             rate_hz = {}\n\
             subjects = {}\n\
             samples = {written}\n",
            source.key,
            source.url,
            source.activity,
            channel.name(),
            source.rate_hz,
            rows.len(),
        ),
    )?;
    Ok(())
}

/// One file found in the archive tree.
struct Entry {
    /// Path inside the archive, lower-cased for matching.
    path: String,
    /// Inflated contents, or `None` when the entry's bytes did not all arrive.
    data: Option<Vec<u8>>,
}

impl Entry {
    fn text(&self) -> Option<String> {
        self.data
            .as_ref()
            .map(|data| String::from_utf8_lossy(data).into_owned())
    }
}

/// Collects every file of the archive tree, descending into nested archives.
///
/// The central directory is not used, because a partially downloaded archive does
/// not have one: the local file headers carry everything needed to decide whether
/// an entry's bytes arrived, which is what makes a half-fetched collection usable.
fn collect_entries(bytes: &[u8], depth: usize) -> Result<Vec<Entry>, Box<dyn std::error::Error>> {
    if depth > 2 {
        return Err("archive nesting is deeper than expected".into());
    }
    // The central directory is authoritative when it is there: an archive written
    // with data descriptors leaves the sizes in the local header at zero and puts
    // them after the data instead, so only the directory knows them. A partial
    // download has no directory, and there the local headers are all there is —
    // which is why both readers exist.
    let headers = match central_headers(bytes) {
        Some(headers) if !headers.is_empty() => headers,
        _ => local_headers(bytes),
    };
    if headers.is_empty() {
        return Err("no zip entry was found".into());
    }
    let mut out = Vec::new();
    for header in &headers {
        if header.name.to_ascii_lowercase().ends_with(".zip") {
            if let Some(inner) = header.read_prefix(bytes)
                && let Ok(nested) = collect_entries(&inner, depth + 1)
            {
                out.extend(nested);
            }
            continue;
        }
        out.push(Entry {
            path: header.name.to_ascii_lowercase(),
            data: header.read(bytes),
        });
    }
    Ok(out)
}

/// Reads one source's rows on one channel.
fn parse(
    source: &Source,
    entries: &[Entry],
    channel: Channel,
) -> Result<Subjects, Box<dyn std::error::Error>> {
    match source.layout {
        Layout::WisimRows => parse_wisdm(entries, channel),
        Layout::MotionSense => parse_motionsense(entries, channel),
        Layout::DailySports => parse_daily_sports(entries),
        Layout::HarSignals => parse_har(entries),
    }
}

/// WISDM: `raw/phone/<channel>/data_<subject>_<channel>_phone.txt`, rows of
/// `subject,activity,timestamp,x,y,z;`.
fn parse_wisdm(
    entries: &[Entry],
    channel: Channel,
) -> Result<Subjects, Box<dyn std::error::Error>> {
    let marker = format!("phone/{}/", channel.name());
    let mut rows: Subjects = BTreeMap::new();
    let mut files = 0usize;
    for entry in entries {
        if !entry.path.contains(&marker) || !entry.path.ends_with(".txt") {
            continue;
        }
        let Some(text) = entry.text() else {
            continue;
        };
        files += 1;
        for line in text.lines() {
            let mut fields = line.trim().split(',');
            let (Some(subject), Some(activity), Some(_timestamp)) =
                (fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            // Jogging is code B in this collection's activity alphabet.
            if !activity
                .trim()
                .trim_end_matches(';')
                .eq_ignore_ascii_case("B")
            {
                continue;
            }
            let Ok(subject) = subject.trim().parse::<u32>() else {
                continue;
            };
            let values: Vec<f64> = fields
                .filter_map(|field| field.trim().trim_end_matches(';').parse::<f64>().ok())
                .collect();
            if values.len() < 3 {
                continue;
            }
            push(&mut rows, subject, [values[0], values[1], values[2]]);
        }
        if rows.len() >= MAX_SUBJECTS {
            break;
        }
    }
    if files == 0 {
        return Err(format!("no {} file is complete in the archive", channel.name()).into());
    }
    Ok(rows)
}

/// MotionSense: `data/A_DeviceMotion_data/<activity>_<trial>/sub_<n>.csv`, a header
/// then `userAcceleration.x/y/z, rotationRate.x/y/z, attitude.…, gravity.…`.
///
/// The subject index appears only in the file name.
fn parse_motionsense(
    entries: &[Entry],
    channel: Channel,
) -> Result<Subjects, Box<dyn std::error::Error>> {
    let wanted: [&str; 3] = match channel {
        Channel::Accel => [
            "useracceleration.x",
            "useracceleration.y",
            "useracceleration.z",
        ],
        Channel::Gyro => ["rotationrate.x", "rotationrate.y", "rotationrate.z"],
    };
    let mut rows: Subjects = BTreeMap::new();
    let mut files = 0usize;
    for entry in entries {
        // The collection keeps its recordings in a nested archive, so the paths
        // here start at the activity directory rather than below some prefix.
        let in_jogging = entry.path.split('/').any(|part| part.starts_with("jog"));
        if !in_jogging || !entry.path.ends_with(".csv") {
            continue;
        }
        let Some(subject) = subject_from_name(&entry.path) else {
            continue;
        };
        let Some(text) = entry.text() else {
            continue;
        };
        let mut lines = text.lines();
        let Some(header) = lines.next() else {
            continue;
        };
        let columns: Vec<String> = header
            .split(',')
            .map(|name| name.trim().to_ascii_lowercase())
            .collect();
        let indices: Option<Vec<usize>> = wanted
            .iter()
            .map(|name| columns.iter().position(|column| column == name))
            .collect();
        let Some(indices) = indices else {
            continue;
        };
        // This collection splits acceleration into a user part with gravity removed
        // and a gravity part. The two are summed, because a gait's signature is
        // measured against the gravity the device also sees: without it the
        // magnitude of the signal is a rectified oscillation and its spectrum peaks
        // at twice the cadence.
        let gravity: Option<Vec<usize>> = ["gravity.x", "gravity.y", "gravity.z"]
            .iter()
            .map(|name| columns.iter().position(|column| column == name))
            .collect();
        let gravity = gravity.filter(|indices| indices.iter().all(|index| *index < columns.len()));
        files += 1;
        for line in lines {
            let values: Vec<f64> = line
                .split(',')
                .map(|field| field.trim().parse::<f64>().unwrap_or(f64::NAN))
                .collect();
            if indices.iter().any(|index| *index >= values.len()) {
                continue;
            }
            let mut sample = [values[indices[0]], values[indices[1]], values[indices[2]]];
            if let Some(gravity) = &gravity {
                if gravity.iter().any(|index| *index >= values.len()) {
                    continue;
                }
                for axis in 0..3 {
                    sample[axis] += values[gravity[axis]];
                }
            }
            if sample.iter().any(|value| !value.is_finite()) {
                continue;
            }
            push(&mut rows, subject, sample);
        }
        if rows.len() >= MAX_SUBJECTS {
            break;
        }
    }
    if files == 0 {
        return Err(format!("no {} file is complete in the archive", channel.name()).into());
    }
    Ok(rows)
}

/// Daily and Sports Activities: `data/<activity>/<subject>/<unit>.txt`, rows of
/// nine comma-separated channels — accelerometer, gyroscope and magnetometer, three
/// axes each — for one body-worn unit at 25 Hz.
///
/// Only the running activities are taken, and only the torso unit's accelerometer:
/// the first three columns of `s01.txt`. The torso is where the step signature is
/// cleanest, and the design's own instrumentation assumption is a device carried on
/// the body rather than on a limb.
fn parse_daily_sports(entries: &[Entry]) -> Result<Subjects, Box<dyn std::error::Error>> {
    let mut rows: Subjects = BTreeMap::new();
    let mut seen: Vec<String> = Vec::new();
    let mut files = 0usize;
    for entry in entries {
        let parts: Vec<&str> = entry.path.split('/').collect();
        let Some(data_index) = parts.iter().position(|part| *part == "data") else {
            continue;
        };
        if parts.len() < data_index + 4 {
            continue;
        }
        // `data/<activity>/<subject>/<unit>.txt`.
        let activity = parts[data_index + 1];
        let subject_name = parts[data_index + 2];
        let unit = parts[data_index + 3];
        if !seen.iter().any(|other| other == activity) {
            seen.push(activity.to_string());
        }
        // a09 and a10 are running on a treadmill at 8 and 12 km/h; the tool prints
        // every activity it saw, so a different ordering is visible rather than
        // silent.
        if activity != "a09" && activity != "a10" || unit != "s01.txt" {
            continue;
        }
        let Ok(subject) = subject_name.trim_start_matches('p').parse::<u32>() else {
            continue;
        };
        let Some(text) = entry.text() else {
            continue;
        };
        files += 1;
        for line in text.lines() {
            let values: Vec<f64> = line
                .split(',')
                .filter_map(|field| field.trim().parse::<f64>().ok())
                .collect();
            if values.len() < 9 {
                continue;
            }
            push(&mut rows, subject, [values[0], values[1], values[2]]);
        }
        if rows.len() >= MAX_SUBJECTS {
            break;
        }
    }
    if files == 0 {
        seen.sort();
        return Err(
            format!("no a09/a10 recording is complete; activities present: {seen:?}").into(),
        );
    }
    Ok(rows)
}

/// HAR: `…/Inertial Signals/body_acc_{x,y,z}_{train,test}.txt`, one 128-sample
/// window per line, at 50 Hz.
///
/// The collection's headline files are pre-extracted feature vectors; the raw
/// windows live beside them under `Inertial Signals`, and only those can calibrate
/// a spectrum. Walking is the only gait it labels, which makes it the reference for
/// the walking mode the design reserves.
fn parse_har(entries: &[Entry]) -> Result<Subjects, Box<dyn std::error::Error>> {
    // One window per line and one file per axis, so a window's sample is assembled
    // from all three. The windows of a split are concatenated into one series: the
    // collection does not label the subject per window, and a gait spectrum does
    // not need it.
    let mut windows: [BTreeMap<(String, usize), Vec<f64>>; 3] = Default::default();
    let mut files = 0usize;
    for entry in entries {
        if !entry.path.contains("inertial signals") {
            continue;
        }
        // The total acceleration, not the body component: this collection ships
        // both and the body one has gravity removed, which leaves a signal whose
        // magnitude is a rectified oscillation — the strongest component of its
        // spectrum sits at twice the cadence, not at the cadence.
        let axis = if entry.path.contains("total_acc_x") {
            0
        } else if entry.path.contains("total_acc_y") {
            1
        } else if entry.path.contains("total_acc_z") {
            2
        } else {
            continue;
        };
        let split = if entry.path.contains("train") {
            "train"
        } else {
            "test"
        };
        let Some(text) = entry.text() else {
            continue;
        };
        files += 1;
        for (window, line) in text.lines().enumerate() {
            let values: Vec<f64> = line
                .split_whitespace()
                .filter_map(|field| field.parse::<f64>().ok())
                .collect();
            if !values.is_empty() {
                windows[axis].insert((split.to_string(), window), values);
            }
        }
    }
    if files == 0 {
        return Err("no raw inertial signal file is complete in the archive".into());
    }

    let mut rows: Subjects = BTreeMap::new();
    let mut splits: Vec<String> = windows[0].keys().map(|(split, _)| split.clone()).collect();
    splits.sort();
    splits.dedup();
    for (index, split) in splits.iter().enumerate() {
        let subject = index as u32 + 1;
        let count = windows
            .iter()
            .flat_map(|axis| axis.keys())
            .filter(|(name, _)| name == split)
            .map(|(_, window)| *window)
            .max()
            .unwrap_or(0);
        // The windows overlap by half, so the first half of each is the part the
        // previous one does not contain. Taking it reconstructs the continuous
        // 50 Hz signal; taking one sample per window would decimate the series to
        // one sample per 2.56 s, whose spectrum says nothing about a gait.
        for window in 0..=count {
            let mut columns: [Option<&Vec<f64>>; 3] = [None, None, None];
            let mut complete = true;
            for (axis, per_axis) in windows.iter().enumerate() {
                match per_axis.get(&(split.clone(), window)) {
                    Some(values) => columns[axis] = Some(values),
                    None => {
                        complete = false;
                        break;
                    }
                }
            }
            if !complete {
                continue;
            }
            let steps = columns[0].map(|values| values.len() / 2).unwrap_or(0);
            for step in 0..steps {
                push(
                    &mut rows,
                    subject,
                    [
                        columns[0].map(|v| v[step]).unwrap_or(0.0),
                        columns[1].map(|v| v[step]).unwrap_or(0.0),
                        columns[2].map(|v| v[step]).unwrap_or(0.0),
                    ],
                );
            }
        }
    }
    Ok(rows)
}

/// Appends one sample, honouring the per-subject cap.
fn push(rows: &mut Subjects, subject: u32, sample: [f64; 3]) {
    let cap = MAX_SAMPLES_PER_SOURCE / MAX_SUBJECTS.max(1);
    let samples = rows.entry(subject).or_default();
    if samples.len() < cap {
        samples.push(sample);
    }
}

/// Subject index from a MotionSense file name, `…/sub_<n>.csv`.
fn subject_from_name(path: &str) -> Option<u32> {
    let name = path.rsplit('/').next()?;
    name.trim_end_matches(".csv")
        .trim_start_matches("sub_")
        .parse::<u32>()
        .ok()
}

/// One entry of a zip's local file headers.
struct Header {
    name: String,
    method: u16,
    compressed: usize,
    start: usize,
}

impl Header {
    /// The entry's bytes as far as they arrived.
    ///
    /// Only for a nested archive, where a prefix is still an archive with its own
    /// local headers: that is what lets a half-downloaded collection be read. A
    /// stored entry can be taken as-is; a truncated deflate stream cannot be
    /// decoded, so a deflated one has to be whole.
    fn read_prefix(&self, archive: &[u8]) -> Option<Vec<u8>> {
        // The slice stops at the entry's declared end. Taking the whole remainder
        // would append whatever follows — for a nested archive that is the outer
        // archive's own directory, whose end-of-directory record then shadows the
        // inner one and makes the whole tree unreadable. A *truncated* download has
        // no declared end to miss, so what arrived is used as it is.
        let end = if self.compressed > 0 {
            (self.start + self.compressed).min(archive.len())
        } else {
            archive.len()
        };
        let body = archive.get(self.start..end)?;
        match self.method {
            0 => Some(body.to_vec()),
            8 if self.compressed > 0 && self.start + self.compressed <= archive.len() => {
                inflate(body).ok()
            }
            _ => None,
        }
    }

    /// The entry's whole contents, or `None` when they did not all arrive.
    fn read(&self, archive: &[u8]) -> Option<Vec<u8>> {
        let body = archive.get(self.start..self.start + self.compressed)?;
        match self.method {
            0 => Some(body.to_vec()),
            8 => inflate(body).ok(),
            _ => None,
        }
    }
}

fn inflate(body: &[u8]) -> Result<Vec<u8>, std::io::Error> {
    let mut out = Vec::new();
    flate2::read::DeflateDecoder::new(body).read_to_end(&mut out)?;
    Ok(out)
}

/// Reads the central directory, which carries the sizes of every entry.
///
/// Returns `None` when there is no directory — a partially downloaded archive
/// does not have one — which leaves the caller to walk the local headers.
fn central_headers(bytes: &[u8]) -> Option<Vec<Header>> {
    let eocd = bytes
        .windows(4)
        .rposition(|window| window == b"PK\x05\x06")?;
    let count = u16::from_le_bytes([bytes[eocd + 10], bytes[eocd + 11]]) as usize;
    let directory = u32::from_le_bytes([
        bytes[eocd + 16],
        bytes[eocd + 17],
        bytes[eocd + 18],
        bytes[eocd + 19],
    ]) as usize;
    let mut out = Vec::with_capacity(count);
    let mut offset = directory;
    for _ in 0..count {
        if bytes.get(offset..offset + 4)? != b"PK\x01\x02" {
            return None;
        }
        let method = u16::from_le_bytes([bytes[offset + 10], bytes[offset + 11]]);
        let compressed = u32::from_le_bytes([
            bytes[offset + 20],
            bytes[offset + 21],
            bytes[offset + 22],
            bytes[offset + 23],
        ]) as usize;
        let name_len = u16::from_le_bytes([bytes[offset + 28], bytes[offset + 29]]) as usize;
        let extra_len = u16::from_le_bytes([bytes[offset + 30], bytes[offset + 31]]) as usize;
        let comment_len = u16::from_le_bytes([bytes[offset + 32], bytes[offset + 33]]) as usize;
        let local_offset = u32::from_le_bytes([
            bytes[offset + 42],
            bytes[offset + 43],
            bytes[offset + 44],
            bytes[offset + 45],
        ]) as usize;
        let name =
            String::from_utf8_lossy(bytes.get(offset + 46..offset + 46 + name_len)?).to_string();
        // The data starts after the *local* header, whose name and extra fields
        // are their own lengths and need not match the directory's.
        let local = local_offset;
        if bytes.get(local..local + 4)? != b"PK\x03\x04" {
            return None;
        }
        let local_name_len = u16::from_le_bytes([bytes[local + 26], bytes[local + 27]]) as usize;
        let local_extra_len = u16::from_le_bytes([bytes[local + 28], bytes[local + 29]]) as usize;
        out.push(Header {
            name,
            method,
            compressed,
            start: local + 30 + local_name_len + local_extra_len,
        });
        offset += 46 + name_len + extra_len + comment_len;
    }
    Some(out)
}

/// Walks an archive's local file headers.
fn local_headers(bytes: &[u8]) -> Vec<Header> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    while offset + 30 <= bytes.len() {
        if bytes[offset..offset + 4] != *b"PK\x03\x04" {
            offset += 1;
            continue;
        }
        let method = u16::from_le_bytes([bytes[offset + 8], bytes[offset + 9]]);
        let compressed = u32::from_le_bytes([
            bytes[offset + 18],
            bytes[offset + 19],
            bytes[offset + 20],
            bytes[offset + 21],
        ]) as usize;
        let name_len = u16::from_le_bytes([bytes[offset + 26], bytes[offset + 27]]) as usize;
        let extra_len = u16::from_le_bytes([bytes[offset + 28], bytes[offset + 29]]) as usize;
        let name = String::from_utf8_lossy(&bytes[offset + 30..offset + 30 + name_len]).to_string();
        let start = offset + 30 + name_len + extra_len;
        out.push(Header {
            name,
            method,
            compressed,
            start,
        });
        offset = start.saturating_add(compressed);
    }
    out
}
