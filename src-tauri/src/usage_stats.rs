use chrono::{DateTime, Local, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    fs::File,
    io::{self, BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::SystemTime,
};

const INDEX_VERSION: u32 = 4;
const METADATA_PREFIX_BYTES: usize = 512;
const MAX_METADATA_LINE_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
struct TokenUsage {
    input_tokens: u64,
    cached_input_tokens: u64,
    output_tokens: u64,
    reasoning_output_tokens: u64,
    total_tokens: u64,
}

impl TokenUsage {
    fn add_assign(&mut self, other: Self) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.cached_input_tokens = self
            .cached_input_tokens
            .saturating_add(other.cached_input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.reasoning_output_tokens = self
            .reasoning_output_tokens
            .saturating_add(other.reasoning_output_tokens);
        self.total_tokens = self.total_tokens.saturating_add(other.total_tokens);
    }

    fn delta_from(self, previous: Self) -> Self {
        let delta = Self {
            input_tokens: counter_delta(self.input_tokens, previous.input_tokens),
            cached_input_tokens: counter_delta(
                self.cached_input_tokens,
                previous.cached_input_tokens,
            ),
            output_tokens: counter_delta(self.output_tokens, previous.output_tokens),
            reasoning_output_tokens: counter_delta(
                self.reasoning_output_tokens,
                previous.reasoning_output_tokens,
            ),
            total_tokens: counter_delta(self.total_tokens, previous.total_tokens),
        };
        if delta.total_tokens == 0 && (delta.input_tokens > 0 || delta.output_tokens > 0) {
            return Self {
                total_tokens: delta.input_tokens.saturating_add(delta.output_tokens),
                ..delta
            };
        }
        delta
    }
}

fn counter_delta(current: u64, previous: u64) -> u64 {
    if current >= previous {
        current - previous
    } else {
        // Resumed and migrated transcripts can restart their cumulative counter.
        current
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct FileUsageIndex {
    observed_len: u64,
    processed_offset: u64,
    modified_millis: u64,
    current_model: String,
    last_total: TokenUsage,
    daily: BTreeMap<String, BTreeMap<String, TokenUsage>>,
    session_id: String,
    session_metadata_seen: bool,
    parent_session_id: Option<String>,
    compressed: bool,
    prefix_hash: String,
    file_identity: String,
    record_ordinal: u64,
    forked: bool,
    events: Vec<UsageEvent>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct UsageEvent {
    replay_id: String,
    timestamp_millis: i64,
    date: String,
    model: String,
    usage: TokenUsage,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct UsageIndex {
    version: u32,
    source_root: String,
    files: BTreeMap<String, FileUsageIndex>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelUsage {
    pub model: String,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_output_tokens: u64,
    pub total_tokens: u64,
}

impl ModelUsage {
    fn from_usage(model: String, usage: TokenUsage) -> Self {
        Self {
            model,
            input_tokens: usage.input_tokens,
            cached_input_tokens: usage.cached_input_tokens,
            output_tokens: usage.output_tokens,
            reasoning_output_tokens: usage.reasoning_output_tokens,
            total_tokens: usage.total_tokens,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyUsage {
    pub date: String,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_output_tokens: u64,
    pub total_tokens: u64,
    pub session_count: u64,
    pub models: Vec<ModelUsage>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageStats {
    pub status: String,
    pub generated_at: String,
    pub first_activity_date: Option<String>,
    pub last_activity_date: Option<String>,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_output_tokens: u64,
    pub total_tokens: u64,
    pub session_count: u64,
    pub active_days: u64,
    pub current_streak: u64,
    pub longest_streak: u64,
    pub indexed_files: u64,
    pub skipped_files: u64,
    pub models: Vec<ModelUsage>,
    pub daily: Vec<DailyUsage>,
    pub official: Option<OfficialUsage>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OfficialUsageBucket {
    pub date: String,
    pub total_tokens: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OfficialUsage {
    pub status: String,
    pub generated_at: String,
    pub as_of: Option<String>,
    pub total_tokens: Option<u64>,
    pub peak_daily_tokens: Option<u64>,
    pub session_count: Option<u64>,
    pub longest_running_turn_seconds: Option<u64>,
    pub current_streak: Option<u64>,
    pub longest_streak: Option<u64>,
    pub daily: Vec<OfficialUsageBucket>,
    pub weekly: Vec<OfficialUsageBucket>,
    pub cumulative: Vec<OfficialUsageBucket>,
}

impl OfficialUsage {
    pub fn unavailable(status: &str) -> Self {
        Self {
            status: status.into(),
            generated_at: Utc::now().to_rfc3339(),
            as_of: None,
            total_tokens: None,
            peak_daily_tokens: None,
            session_count: None,
            longest_running_turn_seconds: None,
            current_streak: None,
            longest_streak: None,
            daily: Vec::new(),
            weekly: Vec::new(),
            cumulative: Vec::new(),
        }
    }
}

impl UsageStats {
    pub fn unavailable() -> Self {
        Self {
            status: "unavailable".into(),
            generated_at: Utc::now().to_rfc3339(),
            first_activity_date: None,
            last_activity_date: None,
            input_tokens: 0,
            cached_input_tokens: 0,
            output_tokens: 0,
            reasoning_output_tokens: 0,
            total_tokens: 0,
            session_count: 0,
            active_days: 0,
            current_streak: 0,
            longest_streak: 0,
            indexed_files: 0,
            skipped_files: 0,
            models: Vec::new(),
            daily: Vec::new(),
            official: None,
        }
    }
}

pub fn load_usage_stats(index_path: &Path) -> Result<UsageStats, String> {
    let Some(codex_home) = codex_home() else {
        return Ok(UsageStats::unavailable());
    };
    if !codex_home.is_dir() {
        return Ok(UsageStats::unavailable());
    }
    load_usage_stats_from_home(&codex_home, index_path)
}

fn load_usage_stats_from_home(codex_home: &Path, index_path: &Path) -> Result<UsageStats, String> {
    let mut index = load_index(index_path);
    let source_root = codex_home.to_string_lossy().into_owned();
    if index.source_root != source_root {
        index = UsageIndex::default();
        index.source_root = source_root;
    }
    let mut paths = Vec::new();
    let mut skipped_files = 0_u64;
    for directory in [
        codex_home.join("sessions"),
        codex_home.join("archived_sessions"),
    ] {
        if directory.exists() {
            collect_session_files(&directory, &mut paths, &mut skipped_files);
        }
    }
    // Compression/archiving can leave both encodings temporarily present.
    // One logical rollout gets one index entry; prefer the live plain file.
    paths.sort_by_key(|path| {
        (
            file_key(path),
            path.extension().is_some_and(|ext| ext == "zst"),
        )
    });
    paths.dedup_by(|right, left| file_key(right) == file_key(left));

    let mut present = BTreeSet::new();
    for path in paths {
        let Some(key) = file_key(&path) else {
            skipped_files = skipped_files.saturating_add(1);
            continue;
        };
        present.insert(key.clone());
        let previous = index.files.get(&key).cloned().unwrap_or_default();
        let mut next = previous.clone();
        if scan_session_file(&path, &mut next).is_err() {
            skipped_files = skipped_files.saturating_add(1);
            if previous.observed_len == 0 {
                index.files.remove(&key);
            }
            continue;
        }
        index.files.insert(key, next);
    }
    // Do not discard known history when a directory was temporarily unreadable.
    if skipped_files == 0 {
        index.files.retain(|key, _| present.contains(key));
    }
    index.version = INDEX_VERSION;

    // A cache failure must not hide valid statistics from the current scan.
    let _ = persist_index(index_path, &index);
    Ok(build_usage_stats(&index, skipped_files))
}

fn codex_home() -> Option<PathBuf> {
    env::var_os("CODEX_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".codex")))
}

fn file_key(path: &Path) -> Option<String> {
    path.file_name()
        .and_then(|value| value.to_str())
        .map(|name| name.strip_suffix(".zst").unwrap_or(name).to_owned())
}

fn collect_session_files(directory: &Path, paths: &mut Vec<PathBuf>, skipped: &mut u64) {
    let Ok(entries) = fs::read_dir(directory) else {
        *skipped = skipped.saturating_add(1);
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            *skipped = skipped.saturating_add(1);
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect_session_files(&path, paths, skipped);
        } else if file_type.is_file()
            && path
                .file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|name| name.ends_with(".jsonl") || name.ends_with(".jsonl.zst"))
        {
            paths.push(path);
        }
    }
}

fn load_index(path: &Path) -> UsageIndex {
    let Ok(file) = File::open(path) else {
        return UsageIndex {
            version: INDEX_VERSION,
            source_root: String::new(),
            files: BTreeMap::new(),
        };
    };
    let Ok(index) = serde_json::from_reader::<_, UsageIndex>(BufReader::new(file)) else {
        return UsageIndex {
            version: INDEX_VERSION,
            source_root: String::new(),
            files: BTreeMap::new(),
        };
    };
    if index.version != INDEX_VERSION
        || index.files.values().any(|file| {
            file.events.iter().any(|event| {
                event.replay_id.len() != 64
                    || !event.replay_id.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
        })
    {
        UsageIndex {
            version: INDEX_VERSION,
            source_root: String::new(),
            files: BTreeMap::new(),
        }
    } else {
        index
    }
}

fn persist_index(path: &Path, index: &UsageIndex) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|_| "failed to create usage index directory")?;
    }
    let temporary = path.with_extension("json.tmp");
    let file = File::create(&temporary).map_err(|_| "failed to create usage index")?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer(&mut writer, index).map_err(|_| "failed to serialize usage index")?;
    writer.flush().map_err(|_| "failed to write usage index")?;
    #[cfg(not(target_os = "windows"))]
    {
        fs::rename(temporary, path).map_err(|_| "failed to commit usage index".into())
    }
    #[cfg(target_os = "windows")]
    {
        let backup = path.with_extension("json.bak");
        let _ = fs::remove_file(&backup);
        if path.exists() {
            fs::rename(path, &backup).map_err(|_| "failed to back up usage index")?;
        }
        if fs::rename(&temporary, path).is_err() {
            let _ = fs::rename(&backup, path);
            return Err("failed to commit usage index".into());
        }
        let _ = fs::remove_file(backup);
        Ok(())
    }
}

fn prefix_hash(path: &Path, len: u64) -> io::Result<String> {
    let mut file = File::open(path)?.take(len);
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    if file.limit() != 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "rollout changed during read",
        ));
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn file_identity(metadata: &fs::Metadata) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        format!(
            "{}:{}:{}:{}",
            metadata.dev(),
            metadata.ino(),
            metadata.ctime(),
            metadata.ctime_nsec()
        )
    }
    #[cfg(not(unix))]
    {
        format!(
            "{:?}:{:?}",
            metadata.created().ok(),
            metadata.modified().ok()
        )
    }
}

fn scan_session_file(path: &Path, entry: &mut FileUsageIndex) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    let current_len = metadata.len();
    let current_modified = modified_millis(&metadata);
    let compressed = path.extension().is_some_and(|ext| ext == "zst");
    let identity = file_identity(&metadata);
    if entry.processed_offset == current_len
        && !compressed
        && entry.observed_len == current_len
        && entry.modified_millis == current_modified
        && entry.file_identity == identity
    {
        return Ok(());
    }
    if compressed
        && entry.compressed
        && entry.observed_len == current_len
        && entry.modified_millis == current_modified
        && entry.file_identity == identity
    {
        return Ok(());
    }

    let can_append = !compressed
        && !entry.compressed
        && entry.observed_len > 0
        && current_len >= entry.observed_len
        && entry.processed_offset <= entry.observed_len
        && (current_len > entry.observed_len || entry.processed_offset < entry.observed_len)
        && prefix_hash(path, entry.observed_len).ok().as_deref()
            == Some(entry.prefix_hash.as_str());
    if !can_append {
        *entry = FileUsageIndex::default();
    }

    let mut file = File::open(path)?;
    let mut reader: Box<dyn BufRead> = if compressed {
        Box::new(BufReader::with_capacity(
            256 * 1024,
            zstd::stream::read::Decoder::new(file)?,
        ))
    } else {
        file.seek(SeekFrom::Start(entry.processed_offset))?;
        Box::new(BufReader::with_capacity(256 * 1024, file))
    };
    let mut captured = Vec::with_capacity(4096);
    let mut relevant: Option<bool> = None;
    let mut absolute_offset = entry.processed_offset;

    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            break;
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let take = newline.map_or(available.len(), |position| position + 1);
        if relevant != Some(false) {
            let chunk = &available[..take];
            let mut copied = 0;
            if relevant.is_none() {
                let probe_remaining = METADATA_PREFIX_BYTES.saturating_sub(captured.len());
                copied = probe_remaining.min(chunk.len());
                captured.extend_from_slice(&chunk[..copied]);
                if captured.len() >= METADATA_PREFIX_BYTES || newline.is_some() {
                    let is_relevant = is_metadata_line(&captured);
                    relevant = Some(is_relevant);
                    if !is_relevant {
                        captured.clear();
                    }
                }
            }
            if relevant == Some(true) && copied < chunk.len() {
                let remaining = MAX_METADATA_LINE_BYTES.saturating_sub(captured.len());
                let tail = &chunk[copied..];
                if tail.len() <= remaining {
                    captured.extend_from_slice(tail);
                } else {
                    captured.clear();
                    relevant = Some(false);
                }
            }
        }
        reader.consume(take);
        absolute_offset = absolute_offset.saturating_add(take as u64);
        if newline.is_some() {
            if relevant == Some(true) {
                process_metadata_line(&captured, entry);
            }
            entry.record_ordinal = entry.record_ordinal.saturating_add(1);
            captured.clear();
            relevant = None;
            entry.processed_offset = absolute_offset;
        }
    }

    let final_metadata = fs::metadata(path)?;
    entry.observed_len = final_metadata.len();
    entry.modified_millis = modified_millis(&final_metadata);
    entry.file_identity = file_identity(&final_metadata);
    entry.compressed = compressed;
    entry.prefix_hash = if compressed {
        String::new()
    } else {
        prefix_hash(path, entry.observed_len)?
    };
    Ok(())
}

fn modified_millis(metadata: &fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

fn process_metadata_line(line: &[u8], entry: &mut FileUsageIndex) {
    let Ok(record) = serde_json::from_slice::<Value>(line) else {
        return;
    };
    let record_type = record
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let payload = record.get("payload").unwrap_or(&Value::Null);
    if record_type == "turn_context" || record_type == "session_meta" {
        if record_type == "session_meta" {
            // Fork history can contain a parent's session_meta after the child's
            // own header. It must not replace the file owner's identity/lineage.
            if entry.session_metadata_seen {
                return;
            }
            let Some(id) = payload
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
            else {
                return;
            };
            entry.session_metadata_seen = true;
            entry.session_id = id.to_owned();
            entry.forked = payload
                .get("forked_from_id")
                .and_then(Value::as_str)
                .is_some();
            entry.parent_session_id = payload
                .get("forked_from_id")
                .or_else(|| payload.get("parent_thread_id"))
                .and_then(Value::as_str)
                .map(str::to_owned);
        }
        if let Some(model) = payload.get("model").and_then(Value::as_str) {
            if !model.trim().is_empty() {
                entry.current_model = model.trim().to_string();
            }
        }
        return;
    }
    if record_type != "event_msg"
        || payload.get("type").and_then(Value::as_str) != Some("token_count")
    {
        return;
    }
    let Some(total) = payload
        .get("info")
        .and_then(|info| info.get("total_token_usage"))
        .map(parse_token_usage)
    else {
        return;
    };
    let mut delta = total.delta_from(entry.last_total);
    if entry.forked && entry.last_total.total_tokens == 0 {
        // A fork can begin with an inherited cumulative baseline without the
        // original events. Only the explicitly recorded request belongs here.
        delta = payload
            .get("info")
            .and_then(|info| info.get("last_token_usage"))
            .map(parse_token_usage)
            .unwrap_or_default();
    }
    entry.last_total = total;
    // Retain zero-delta snapshots too: a child may replay a parent's restored
    // counter, and that snapshot is evidence of inheritance, not a new request.
    // Projection ordinals are deliberately not used as accounting boundaries.
    let Some(timestamp) = record
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
    else {
        return;
    };
    let date = timestamp
        .with_timezone(&Local)
        .format("%Y-%m-%d")
        .to_string();
    let model = if entry.current_model.trim().is_empty() {
        "unknown"
    } else {
        entry.current_model.as_str()
    };
    entry
        .daily
        .entry(date.clone())
        .or_default()
        .entry(model.to_string())
        .or_default()
        .add_assign(delta);
    let mut replay_digest = Sha256::new();
    for value in [
        total.input_tokens,
        total.cached_input_tokens,
        total.output_tokens,
        total.reasoning_output_tokens,
        total.total_tokens,
    ] {
        replay_digest.update(value.to_le_bytes());
    }
    let last = payload
        .get("info")
        .and_then(|info| info.get("last_token_usage"));
    replay_digest.update([u8::from(last.is_some_and(Value::is_object))]);
    let last = last.map(parse_token_usage).unwrap_or_default();
    for value in [
        last.input_tokens,
        last.cached_input_tokens,
        last.output_tokens,
        last.reasoning_output_tokens,
        last.total_tokens,
    ] {
        replay_digest.update(value.to_le_bytes());
    }
    entry.events.push(UsageEvent {
        replay_id: format!("{:x}", replay_digest.finalize()),
        timestamp_millis: timestamp.timestamp_millis(),
        date,
        model: model.to_owned(),
        usage: delta,
    });
}

fn is_metadata_line(prefix: &[u8]) -> bool {
    contains_bytes(prefix, b"turn_context")
        || contains_bytes(prefix, b"session_meta")
        || contains_bytes(prefix, b"token_count")
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|candidate| candidate == needle)
}

fn parse_token_usage(value: &Value) -> TokenUsage {
    TokenUsage {
        input_tokens: value
            .get("input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        cached_input_tokens: value
            .get("cached_input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        output_tokens: value
            .get("output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        reasoning_output_tokens: value
            .get("reasoning_output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        total_tokens: value
            .get("total_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    }
}

fn build_usage_stats(index: &UsageIndex, skipped_files: u64) -> UsageStats {
    let mut daily: BTreeMap<String, (TokenUsage, u64, BTreeMap<String, TokenUsage>)> =
        BTreeMap::new();
    let mut session_count = 0_u64;

    let mut seen_events = BTreeSet::new();
    let mut seen_sessions = BTreeSet::new();
    let mut day_sessions: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    // Only forked_from_id means inherited history. A fresh subagent's
    // parent_thread_id is ownership, not evidence that its usage was replayed.
    let parents: BTreeMap<_, _> = index
        .files
        .values()
        .filter(|file| file.forked)
        .filter_map(|file| Some((file.session_id.as_str(), file.parent_session_id.as_deref()?)))
        .collect();
    let mut snapshots: BTreeMap<&str, BTreeMap<&str, i64>> = BTreeMap::new();
    for file in index.files.values() {
        if file.session_id.is_empty() {
            continue;
        }
        let session = snapshots.entry(&file.session_id).or_default();
        for event in &file.events {
            session
                .entry(&event.replay_id)
                .and_modify(|time| *time = (*time).min(event.timestamp_millis))
                .or_insert(event.timestamp_millis);
        }
    }
    // Prefer original timestamps when a copied rollout was re-timestamped, then
    // merge additional occurrences from longer copies instead of losing suffixes.
    let mut files: Vec<_> = index.files.iter().collect();
    files.sort_by_key(|(key, file)| {
        (
            file.events
                .first()
                .map_or(i64::MAX, |event| event.timestamp_millis),
            std::cmp::Reverse(file.events.len()),
            *key,
        )
    });
    for (key, file) in files {
        let session = if file.session_id.is_empty() {
            key
        } else {
            &file.session_id
        };
        let mut ancestors = Vec::new();
        let mut ancestor = parents.get(session.as_str()).copied();
        let mut visited = BTreeSet::from([session.as_str()]);
        while let Some(parent) = ancestor {
            // Corrupt relationship cycles cannot establish inheritance.
            if !visited.insert(parent) {
                ancestors.clear();
                break;
            }
            ancestors.push(parent);
            ancestor = parents.get(parent).copied();
        }
        let mut inherited_prefix = file.forked && !ancestors.is_empty();
        let mut occurrences: BTreeMap<&str, u64> = BTreeMap::new();
        for event in &file.events {
            if inherited_prefix {
                let inherited = ancestors.iter().any(|parent| {
                    snapshots
                        .get(parent)
                        .and_then(|events| events.get(event.replay_id.as_str()))
                        .is_some_and(|time| *time <= event.timestamp_millis)
                });
                if inherited {
                    continue;
                }
                // Do not merge later real requests just because a counter was
                // reset and happens to equal an earlier ancestor's counter.
                if event.usage.total_tokens > 0 {
                    inherited_prefix = false;
                }
            }
            if event.usage.total_tokens == 0 {
                continue;
            }
            let occurrence = occurrences.entry(&event.replay_id).or_default();
            *occurrence += 1;
            // Copies of one session share identities even if re-timestamped.
            // Multiplicity preserves legitimate repeated requests after resets;
            // different sessions (including siblings) are never merged here.
            if !seen_events.insert((session.as_str(), event.replay_id.as_str(), *occurrence)) {
                continue;
            }
            seen_sessions.insert(session);
            day_sessions
                .entry(event.date.clone())
                .or_default()
                .insert(session.clone());
            let target = daily.entry(event.date.clone()).or_default();
            target.0.add_assign(event.usage);
            target
                .2
                .entry(event.model.clone())
                .or_default()
                .add_assign(event.usage);
        }
    }
    session_count += seen_sessions.len() as u64;
    for (date, entry) in &mut daily {
        entry.1 = day_sessions
            .get(date)
            .map_or(0, |sessions| sessions.len() as u64);
    }

    let active_dates: Vec<NaiveDate> = daily
        .keys()
        .filter_map(|date| NaiveDate::parse_from_str(date, "%Y-%m-%d").ok())
        .collect();
    let (current_streak, longest_streak) = streaks(&active_dates, Local::now().date_naive());
    let first_activity_date = daily.keys().next().cloned();
    let last_activity_date = daily.keys().next_back().cloned();
    let mut overall = TokenUsage::default();
    let mut overall_models: BTreeMap<String, TokenUsage> = BTreeMap::new();
    let mut daily_rows = Vec::with_capacity(daily.len());

    for (date, (usage, sessions, model_map)) in daily {
        overall.add_assign(usage);
        let mut models: Vec<ModelUsage> = model_map
            .into_iter()
            .map(|(model, model_usage)| {
                overall_models
                    .entry(model.clone())
                    .or_default()
                    .add_assign(model_usage);
                ModelUsage::from_usage(model, model_usage)
            })
            .collect();
        models.sort_by(|left, right| right.total_tokens.cmp(&left.total_tokens));
        daily_rows.push(DailyUsage {
            date,
            input_tokens: usage.input_tokens,
            cached_input_tokens: usage.cached_input_tokens,
            output_tokens: usage.output_tokens,
            reasoning_output_tokens: usage.reasoning_output_tokens,
            total_tokens: usage.total_tokens,
            session_count: sessions,
            models,
        });
    }

    let mut models: Vec<ModelUsage> = overall_models
        .into_iter()
        .map(|(model, usage)| ModelUsage::from_usage(model, usage))
        .collect();
    models.sort_by(|left, right| right.total_tokens.cmp(&left.total_tokens));

    UsageStats {
        status: if overall.total_tokens > 0 {
            "ok"
        } else {
            "empty"
        }
        .into(),
        generated_at: Utc::now().to_rfc3339(),
        first_activity_date,
        last_activity_date,
        input_tokens: overall.input_tokens,
        cached_input_tokens: overall.cached_input_tokens,
        output_tokens: overall.output_tokens,
        reasoning_output_tokens: overall.reasoning_output_tokens,
        total_tokens: overall.total_tokens,
        session_count,
        active_days: active_dates.len() as u64,
        current_streak,
        longest_streak,
        indexed_files: index.files.len() as u64,
        skipped_files,
        models,
        daily: daily_rows,
        official: None,
    }
}

fn streaks(active_dates: &[NaiveDate], today: NaiveDate) -> (u64, u64) {
    if active_dates.is_empty() {
        return (0, 0);
    }
    let mut longest = 1_u64;
    let mut run = 1_u64;
    for dates in active_dates.windows(2) {
        if dates[1].signed_duration_since(dates[0]).num_days() == 1 {
            run = run.saturating_add(1);
            longest = longest.max(run);
        } else {
            run = 1;
        }
    }
    let last = *active_dates.last().unwrap_or(&today);
    if today.signed_duration_since(last).num_days() > 1 {
        return (0, longest);
    }
    let mut current = 1_u64;
    for dates in active_dates.windows(2).rev() {
        if dates[1].signed_duration_since(dates[0]).num_days() == 1 {
            current = current.saturating_add(1);
        } else {
            break;
        }
    }
    (current, longest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::OpenOptions;

    fn fixture_dir() -> PathBuf {
        static NEXT_FIXTURE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = env::temp_dir().join(format!(
            "halo-usage-{}-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap(),
            NEXT_FIXTURE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir_all(path.join("sessions")).unwrap();
        path
    }

    fn fixture_log(total: u64) -> Vec<u8> {
        let mut data =
            br#"{"type":"session_meta","payload":{"id":"session-one","model":"gpt-test"}}"#
                .to_vec();
        data.push(b'\n');
        data.extend(token_line("2026-07-10T12:00:00Z", total - 10, 10, total));
        data.push(b'\n');
        data
    }

    #[test]
    fn compression_preserves_history_and_never_counts_both_encodings() {
        let root = fixture_dir();
        let plain = root.join("sessions/rollout.jsonl");
        let packed = root.join("sessions/rollout.jsonl.zst");
        let index = root.join("index.json");
        let data = fixture_log(100);
        fs::write(&plain, &data).unwrap();
        assert_eq!(
            load_usage_stats_from_home(&root, &index)
                .unwrap()
                .total_tokens,
            100
        );
        fs::write(
            &packed,
            zstd::stream::encode_all(data.as_slice(), 1).unwrap(),
        )
        .unwrap();
        assert_eq!(
            load_usage_stats_from_home(&root, &index)
                .unwrap()
                .total_tokens,
            100
        );
        fs::remove_file(plain).unwrap();
        let stats = load_usage_stats_from_home(&root, &index).unwrap();
        assert_eq!(
            (stats.total_tokens, stats.indexed_files, stats.skipped_files),
            (100, 1, 0)
        );
        assert_eq!(
            load_usage_stats_from_home(&root, &index)
                .unwrap()
                .total_tokens,
            100
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn copied_rollouts_and_replayed_events_are_deduplicated() {
        let root = fixture_dir();
        fs::create_dir_all(root.join("archived_sessions")).unwrap();
        let data = fixture_log(100);
        fs::write(root.join("sessions/original.jsonl"), &data).unwrap();
        fs::write(
            root.join("archived_sessions/copy.jsonl.zst"),
            zstd::stream::encode_all(data.as_slice(), 1).unwrap(),
        )
        .unwrap();
        let stats = load_usage_stats_from_home(&root, &root.join("index.json")).unwrap();
        assert_eq!((stats.total_tokens, stats.session_count), (100, 1));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn independent_sessions_with_equal_counters_are_not_merged() {
        let root = fixture_dir();
        let first = fixture_log(100);
        let second = String::from_utf8(first.clone())
            .unwrap()
            .replace("session-one", "session-two");
        fs::write(root.join("sessions/a.jsonl"), first).unwrap();
        fs::write(root.join("sessions/b.jsonl"), second).unwrap();
        let stats = load_usage_stats_from_home(&root, &root.join("index.json")).unwrap();
        assert_eq!((stats.total_tokens, stats.session_count), (200, 2));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fork_replay_deduplicates_against_its_parent_lineage() {
        let root = fixture_dir();
        fs::write(root.join("sessions/parent.jsonl"), fixture_log(100)).unwrap();
        let mut child = br#"{"type":"session_meta","payload":{"id":"child","model":"gpt-test","forked_from_id":"session-one"}}"#.to_vec();
        child.push(b'\n');
        child.extend(token_line("2026-07-10T12:00:00Z", 90, 10, 100));
        child.push(b'\n');
        child.extend(token_line("2026-07-10T12:01:00Z", 110, 20, 130));
        child.push(b'\n');
        fs::write(root.join("sessions/child.jsonl"), child).unwrap();
        assert_eq!(
            load_usage_stats_from_home(&root, &root.join("index.json"))
                .unwrap()
                .total_tokens,
            130
        );
        fs::remove_dir_all(root).unwrap();
    }

    fn session_with_requests(
        id: &str,
        parent: Option<&str>,
        events: &[(&str, u64, u64)],
    ) -> Vec<u8> {
        let meta = serde_json::json!({"type":"session_meta","payload":{
            "id":id,"forked_from_id":parent,"model":"gpt-test"
        }});
        let mut data = format!("{meta}\n").into_bytes();
        for (timestamp, total, last) in events {
            let row = serde_json::json!({"timestamp":timestamp,"type":"event_msg","payload":{
                "type":"token_count","info":{
                    "total_token_usage":{"input_tokens":total,"total_tokens":total},
                    "last_token_usage":{"input_tokens":last,"total_tokens":last}
                }
            }});
            data.extend(format!("{row}\n").as_bytes());
        }
        data
    }

    #[test]
    fn retimestamped_fork_prefix_is_removed_but_new_requests_are_kept() {
        let root = fixture_dir();
        let parent = session_with_requests(
            "parent",
            None,
            &[
                ("2026-07-01T10:00:00Z", 100, 100),
                ("2026-07-01T10:01:00Z", 150, 50),
            ],
        );
        let child = session_with_requests(
            "child",
            Some("parent"),
            &[
                ("2026-08-01T10:00:00Z", 100, 100),
                ("2026-08-01T10:00:00Z", 150, 50),
                ("2026-08-01T10:01:00Z", 180, 30),
            ],
        );
        fs::write(root.join("sessions/parent.jsonl"), parent).unwrap();
        fs::write(
            root.join("sessions/child.jsonl.zst"),
            zstd::stream::encode_all(child.as_slice(), 1).unwrap(),
        )
        .unwrap();
        let stats = load_usage_stats_from_home(&root, &root.join("index.json")).unwrap();
        assert_eq!(stats.total_tokens, 180);
        assert_eq!(
            stats
                .daily
                .iter()
                .find(|d| d.date == "2026-07-01")
                .unwrap()
                .total_tokens,
            150
        );
        assert_eq!(
            stats
                .daily
                .iter()
                .find(|d| d.date == "2026-08-01")
                .unwrap()
                .total_tokens,
            30
        );
        assert_eq!(stats.session_count, 2);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn inherited_session_metadata_cannot_replace_the_child_header() {
        let root = fixture_dir();
        let parent = session_with_requests("parent", None, &[("2026-07-01T10:00:00Z", 100, 100)]);
        let mut child = session_with_requests("child", Some("parent"), &[]);
        child.extend(session_with_requests(
            "parent",
            None,
            &[
                ("2026-07-02T10:00:00Z", 100, 100),
                ("2026-07-02T10:01:00Z", 130, 30),
            ],
        ));
        fs::write(root.join("sessions/parent.jsonl"), parent).unwrap();
        fs::write(root.join("sessions/child.jsonl"), child).unwrap();
        let index_path = root.join("index.json");
        let stats = load_usage_stats_from_home(&root, &index_path).unwrap();
        assert_eq!((stats.total_tokens, stats.session_count), (130, 2));
        let index = load_index(&index_path);
        assert_eq!(index.files["child.jsonl"].session_id, "child");
        assert!(index.files["child.jsonl"].forked);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn grandparent_history_is_removed_and_equal_sibling_requests_remain_separate() {
        let root = fixture_dir();
        for (name, data) in [
            (
                "root",
                session_with_requests("root", None, &[("2026-07-01T10:00:00Z", 100, 100)]),
            ),
            (
                "child",
                session_with_requests(
                    "child",
                    Some("root"),
                    &[
                        ("2026-07-02T10:00:00Z", 100, 100),
                        ("2026-07-02T10:01:00Z", 150, 50),
                    ],
                ),
            ),
            (
                "a",
                session_with_requests(
                    "a",
                    Some("child"),
                    &[
                        ("2026-07-03T10:00:00Z", 100, 100),
                        ("2026-07-03T10:00:00Z", 150, 50),
                        ("2026-07-03T10:01:00Z", 170, 20),
                    ],
                ),
            ),
            (
                "b",
                session_with_requests(
                    "b",
                    Some("child"),
                    &[
                        ("2026-07-03T10:00:00Z", 100, 100),
                        ("2026-07-03T10:00:00Z", 150, 50),
                        ("2026-07-03T10:01:00Z", 170, 20),
                    ],
                ),
            ),
        ] {
            fs::write(root.join(format!("sessions/{name}.jsonl")), data).unwrap();
        }
        let stats = load_usage_stats_from_home(&root, &root.join("index.json")).unwrap();
        assert_eq!((stats.total_tokens, stats.session_count), (190, 4));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_real_counter_restart_after_the_inherited_prefix_is_not_removed() {
        let root = fixture_dir();
        fs::write(
            root.join("sessions/parent.jsonl"),
            session_with_requests("parent", None, &[("2026-07-01T10:00:00Z", 100, 100)]),
        )
        .unwrap();
        fs::write(
            root.join("sessions/child.jsonl"),
            session_with_requests(
                "child",
                Some("parent"),
                &[
                    ("2026-07-02T10:00:00Z", 100, 100),
                    ("2026-07-02T10:01:00Z", 150, 50),
                    ("2026-07-02T10:02:00Z", 100, 100),
                ],
            ),
        )
        .unwrap();
        let stats = load_usage_stats_from_home(&root, &root.join("index.json")).unwrap();
        assert_eq!(stats.total_tokens, 250);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn retimestamped_session_copies_preserve_original_days_and_request_multiplicity() {
        let root = fixture_dir();
        let original = session_with_requests(
            "same",
            None,
            &[
                ("2026-07-01T10:00:00Z", 100, 100),
                ("2026-07-01T10:01:00Z", 30, 30),
                ("2026-07-01T10:02:00Z", 100, 100),
            ],
        );
        let copy = String::from_utf8(original.clone())
            .unwrap()
            .replace("2026-07-01", "2026-08-01");
        fs::write(root.join("sessions/z-original.jsonl"), original).unwrap();
        fs::write(root.join("sessions/a-copy.jsonl"), copy).unwrap();
        let stats = load_usage_stats_from_home(&root, &root.join("index.json")).unwrap();
        // The third request adds 70, rather than erasing the legitimate restart.
        assert_eq!(
            (stats.total_tokens, stats.session_count, stats.daily.len()),
            (200, 1, 1)
        );
        assert_eq!(stats.daily[0].date, "2026-07-01");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ownership_only_is_not_treated_as_inherited_history() {
        let root = fixture_dir();
        let parent = session_with_requests("parent", None, &[("2026-07-01T10:00:00Z", 100, 100)]);
        let child = String::from_utf8(session_with_requests(
            "child",
            Some("parent"),
            &[("2026-07-02T10:00:00Z", 100, 100)],
        ))
        .unwrap()
        .replace("forked_from_id", "parent_thread_id");
        fs::write(root.join("sessions/parent.jsonl"), parent).unwrap();
        fs::write(root.join("sessions/child.jsonl"), child).unwrap();
        assert_eq!(
            load_usage_stats_from_home(&root, &root.join("index.json"))
                .unwrap()
                .total_tokens,
            200
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_new_earlier_request_is_not_erased_by_a_later_parent_snapshot() {
        let root = fixture_dir();
        fs::write(
            root.join("sessions/parent.jsonl"),
            session_with_requests("parent", None, &[("2026-07-02T10:00:00Z", 100, 100)]),
        )
        .unwrap();
        fs::write(
            root.join("sessions/child.jsonl"),
            session_with_requests(
                "child",
                Some("parent"),
                &[("2026-07-01T10:00:00Z", 100, 100)],
            ),
        )
        .unwrap();
        assert_eq!(
            load_usage_stats_from_home(&root, &root.join("index.json"))
                .unwrap()
                .total_tokens,
            200
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corrupt_parent_cycles_cannot_erase_both_sessions() {
        let root = fixture_dir();
        for (id, parent) in [("a", "b"), ("b", "a")] {
            fs::write(
                root.join(format!("sessions/{id}.jsonl")),
                session_with_requests(id, Some(parent), &[("2026-07-01T10:00:00Z", 100, 100)]),
            )
            .unwrap();
        }
        assert_eq!(
            load_usage_stats_from_home(&root, &root.join("index.json"))
                .unwrap()
                .total_tokens,
            200
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn old_index_versions_are_rebuilt_not_reused_with_empty_fingerprints() {
        let root = fixture_dir();
        let index = root.join("index.json");
        fs::write(root.join("sessions/a.jsonl"), fixture_log(100)).unwrap();
        load_usage_stats_from_home(&root, &index).unwrap();
        let mut cache: Value = serde_json::from_slice(&fs::read(&index).unwrap()).unwrap();
        cache["version"] = serde_json::json!(3);
        for file in cache["files"].as_object_mut().unwrap().values_mut() {
            file["events"] = serde_json::json!([]);
        }
        fs::write(&index, serde_json::to_vec(&cache).unwrap()).unwrap();
        assert_eq!(
            load_usage_stats_from_home(&root, &index)
                .unwrap()
                .total_tokens,
            100
        );
        assert_eq!(load_index(&index).version, 4);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn migrated_projection_boundary_at_eof_does_not_hide_real_requests() {
        let root = fixture_dir();
        let data = serde_json::json!({"type":"session_meta","payload":{
            "id":"child", "forked_from_id":"parent", "model":"gpt-test",
            "subagent_history_start_ordinal":3, "history_mode":"paginated"
        }});
        let request = serde_json::json!({"timestamp":"2026-07-10T12:01:00Z","type":"event_msg","payload":{
            "type":"token_count", "info":{
                "total_token_usage":{"input_tokens":90,"output_tokens":10,"total_tokens":100},
                "last_token_usage":{"input_tokens":90,"output_tokens":10,"total_tokens":100}
            }
        }});
        fs::write(
            root.join("sessions/child.jsonl"),
            format!("{data}\n{request}\n"),
        )
        .unwrap();
        assert_eq!(
            load_usage_stats_from_home(&root, &root.join("index.json"))
                .unwrap()
                .total_tokens,
            100
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fork_history_sets_the_baseline_without_recounting_parent_tokens() {
        let root = fixture_dir();
        let mut data = br#"{"type":"session_meta","payload":{"id":"child","model":"gpt-test","forked_from_id":"parent","subagent_history_start_ordinal":2}}"#.to_vec();
        data.push(b'\n');
        data.extend(token_line("2026-07-10T12:00:00Z", 90, 10, 100));
        data.push(b'\n');
        data.extend(token_line("2026-07-10T12:01:00Z", 110, 20, 130));
        data.push(b'\n');
        fs::write(root.join("sessions/child.jsonl"), data).unwrap();
        let stats = load_usage_stats_from_home(&root, &root.join("index.json")).unwrap();
        assert_eq!(stats.total_tokens, 30);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rewritten_file_that_grows_rebuilds_instead_of_appending_to_stale_counts() {
        let root = fixture_dir();
        let path = root.join("sessions/rewrite.jsonl");
        fs::write(&path, fixture_log(999)).unwrap();
        let mut index = FileUsageIndex::default();
        scan_session_file(&path, &mut index).unwrap();
        let mut replacement = fixture_log(100);
        replacement.extend(std::iter::repeat_n(b' ', 100));
        replacement.push(b'\n');
        fs::write(&path, replacement).unwrap();
        scan_session_file(&path, &mut index).unwrap();
        assert_eq!(
            index
                .events
                .iter()
                .map(|e| e.usage.total_tokens)
                .sum::<u64>(),
            100
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_compression_is_reported_without_removing_previous_aggregate() {
        let root = fixture_dir();
        let path = root.join("sessions/rollout.jsonl.zst");
        fs::write(
            &path,
            zstd::stream::encode_all(fixture_log(100).as_slice(), 1).unwrap(),
        )
        .unwrap();
        let index = root.join("index.json");
        load_usage_stats_from_home(&root, &index).unwrap();
        fs::write(&path, b"not zstandard").unwrap();
        let stats = load_usage_stats_from_home(&root, &index).unwrap();
        assert_eq!((stats.total_tokens, stats.skipped_files), (100, 1));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_index_cannot_leak_counts_between_codex_home_directories() {
        let first = fixture_dir();
        let second = fixture_dir();
        fs::write(first.join("sessions/a.jsonl"), fixture_log(100)).unwrap();
        fs::write(second.join("sessions/b.jsonl"), fixture_log(200)).unwrap();
        let index = first.join("index.json");
        assert_eq!(
            load_usage_stats_from_home(&first, &index)
                .unwrap()
                .total_tokens,
            100
        );
        assert_eq!(
            load_usage_stats_from_home(&second, &index)
                .unwrap()
                .total_tokens,
            200
        );
        fs::remove_dir_all(first).unwrap();
        fs::remove_dir_all(second).unwrap();
    }

    #[test]
    #[ignore = "reads local counters only; run explicitly for desktop verification"]
    fn live_local_usage_rebuild_and_warm_cache() {
        let root = fixture_dir();
        let path = root.join("live-index.json");
        let started = std::time::Instant::now();
        let first = load_usage_stats(&path).unwrap();
        assert_eq!(first.skipped_files, 0);
        assert!(first.indexed_files > 0);
        println!(
            "local rebuild: files={}, sessions={}, tokens={}, seconds={:.2}",
            first.indexed_files,
            first.session_count,
            first.total_tokens,
            started.elapsed().as_secs_f64()
        );
        let frozen = load_index(&path);
        let before_dedupe: u64 = frozen
            .files
            .values()
            .flat_map(|file| &file.events)
            .map(|event| event.usage.total_tokens)
            .sum();
        assert!(first.total_tokens <= before_dedupe);
        println!(
            "real-log accounting: snapshots={}, before_dedupe={}, removed={}",
            frozen
                .files
                .values()
                .map(|file| file.events.len())
                .sum::<usize>(),
            before_dedupe,
            before_dedupe - first.total_tokens
        );
        for month in ["2026-07", "2026-08", "2026-09"] {
            println!(
                "real-log month {month}: {}",
                first
                    .daily
                    .iter()
                    .filter(|day| day.date.starts_with(month))
                    .map(|day| day.total_tokens)
                    .sum::<u64>()
            );
        }
        // Verify idempotence against the actual frozen counter snapshot, without
        // editing any source log: add re-timestamped copies to an in-memory index.
        let mut duplicated = load_index(&path);
        for (key, file) in &frozen.files {
            let mut copy = file.clone();
            for event in &mut copy.events {
                event.timestamp_millis += 86_400_000;
                event.date = NaiveDate::parse_from_str(&event.date, "%Y-%m-%d")
                    .unwrap()
                    .succ_opt()
                    .unwrap()
                    .to_string();
            }
            duplicated.files.insert(format!("copy-{key}"), copy);
        }
        let copied = build_usage_stats(&duplicated, 0);
        assert_eq!(copied.total_tokens, first.total_tokens);
        assert_eq!(
            serde_json::to_value(&copied.models).unwrap(),
            serde_json::to_value(&first.models).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&copied.daily).unwrap(),
            serde_json::to_value(&first.daily).unwrap()
        );
        println!(
            "real-log replay idempotence: {} re-timestamped copies, totals/models/days unchanged",
            frozen.files.len()
        );
        let warm = std::time::Instant::now();
        let second = load_usage_stats(&path).unwrap();
        assert!(second.total_tokens >= first.total_tokens);
        println!(
            "warm cache: tokens={}, seconds={:.2}",
            second.total_tokens,
            warm.elapsed().as_secs_f64()
        );
        fs::remove_dir_all(root).unwrap();
    }

    fn token_line(timestamp: &str, input: u64, output: u64, total: u64) -> Vec<u8> {
        serde_json::json!({
            "timestamp": timestamp,
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "total_token_usage": {
                        "input_tokens": input,
                        "cached_input_tokens": 0,
                        "output_tokens": output,
                        "reasoning_output_tokens": 0,
                        "total_tokens": total
                    }
                }
            }
        })
        .to_string()
        .into_bytes()
    }

    #[test]
    fn cumulative_events_are_deduplicated_and_attributed_to_the_active_model() {
        let mut entry = FileUsageIndex::default();
        process_metadata_line(
            br#"{"type":"turn_context","payload":{"model":"gpt-5.4"}}"#,
            &mut entry,
        );
        process_metadata_line(&token_line("2026-07-10T12:00:00Z", 90, 10, 100), &mut entry);
        process_metadata_line(&token_line("2026-07-10T12:00:01Z", 90, 10, 100), &mut entry);
        process_metadata_line(
            &token_line("2026-07-10T12:00:02Z", 150, 20, 170),
            &mut entry,
        );

        let usage = entry.daily.values().next().unwrap().get("gpt-5.4").unwrap();
        assert_eq!(usage.input_tokens, 150);
        assert_eq!(usage.output_tokens, 20);
        assert_eq!(usage.total_tokens, 170);
    }

    #[test]
    fn restarted_cumulative_counter_starts_a_new_segment() {
        let mut entry = FileUsageIndex::default();
        process_metadata_line(
            br#"{"type":"turn_context","payload":{"model":"gpt-5.4"}}"#,
            &mut entry,
        );
        process_metadata_line(&token_line("2026-07-10T12:00:00Z", 90, 10, 100), &mut entry);
        process_metadata_line(&token_line("2026-07-10T12:01:00Z", 25, 5, 30), &mut entry);
        let usage = entry.daily.values().next().unwrap().get("gpt-5.4").unwrap();
        assert_eq!(usage.total_tokens, 130);
    }

    #[test]
    fn streaks_keep_yesterdays_run_current_but_expire_older_runs() {
        let today = NaiveDate::from_ymd_opt(2026, 7, 15).unwrap();
        let dates = vec![
            NaiveDate::from_ymd_opt(2026, 7, 10).unwrap(),
            NaiveDate::from_ymd_opt(2026, 7, 11).unwrap(),
            NaiveDate::from_ymd_opt(2026, 7, 12).unwrap(),
            NaiveDate::from_ymd_opt(2026, 7, 14).unwrap(),
        ];
        assert_eq!(streaks(&dates, today), (1, 3));
        assert_eq!(streaks(&dates[..3], today), (0, 3));
    }

    #[test]
    fn file_index_only_processes_appended_bytes() {
        let path = std::env::temp_dir().join(format!(
            "codex-halo-usage-index-test-{}-{}.jsonl",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let mut initial = br#"{"type":"turn_context","payload":{"model":"gpt-5.4"}}"#.to_vec();
        initial.push(b'\n');
        initial.extend(token_line("2026-07-10T12:00:00Z", 90, 10, 100));
        initial.push(b'\n');
        fs::write(&path, initial).unwrap();

        let mut index = FileUsageIndex::default();
        scan_session_file(&path, &mut index).unwrap();
        let first_total = index
            .daily
            .values()
            .next()
            .unwrap()
            .get("gpt-5.4")
            .unwrap()
            .total_tokens;
        assert_eq!(first_total, 100);

        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(&token_line("2026-07-10T12:01:00Z", 150, 20, 170))
            .unwrap();
        file.write_all(b"\n").unwrap();
        file.flush().unwrap();
        scan_session_file(&path, &mut index).unwrap();
        scan_session_file(&path, &mut index).unwrap();

        let final_total = index
            .daily
            .values()
            .next()
            .unwrap()
            .get("gpt-5.4")
            .unwrap()
            .total_tokens;
        assert_eq!(final_total, 170);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn truncated_file_rebuilds_the_cached_aggregate() {
        let path = std::env::temp_dir().join(format!(
            "codex-halo-usage-truncation-test-{}-{}.jsonl",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let mut initial = br#"{"type":"turn_context","payload":{"model":"gpt-5.4"}}"#.to_vec();
        initial.push(b'\n');
        initial.extend(token_line("2026-07-10T12:00:00Z", 90, 10, 100));
        initial.push(b'\n');
        initial.extend(std::iter::repeat_n(b'x', 2048));
        fs::write(&path, initial).unwrap();

        let mut index = FileUsageIndex::default();
        scan_session_file(&path, &mut index).unwrap();
        assert_eq!(
            index
                .daily
                .values()
                .next()
                .unwrap()
                .get("gpt-5.4")
                .unwrap()
                .total_tokens,
            100
        );

        let mut replacement = br#"{"type":"turn_context","payload":{"model":"gpt-5.4"}}"#.to_vec();
        replacement.push(b'\n');
        replacement.extend(token_line("2026-07-10T12:01:00Z", 25, 5, 30));
        replacement.push(b'\n');
        fs::write(&path, replacement).unwrap();
        scan_session_file(&path, &mut index).unwrap();

        assert_eq!(
            index
                .daily
                .values()
                .next()
                .unwrap()
                .get("gpt-5.4")
                .unwrap()
                .total_tokens,
            30
        );
        let _ = fs::remove_file(path);
    }
}
