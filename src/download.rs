use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    ffi::{c_char, c_int, c_void, CStr, CString},
    fs::{self, File, OpenOptions},
    io,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
pub type Shared = Arc<Mutex<Queue>>;

#[derive(Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub url: String,
    pub destination: PathBuf,
    pub status: String,
    pub received: u64,
    pub total: Option<u64>,
    token: Option<String>,
    resource: Option<serde_json::Value>,
    pub error: String,
}

impl Job {
    fn sidecar(&self, extension: &str) -> PathBuf {
        self.destination
            .with_file_name(format!(".relay-{}.{extension}", self.id))
    }
}

pub struct Queue {
    path: PathBuf,
    pub jobs: Vec<Job>,
    workers: HashMap<String, Arc<AtomicBool>>,
    closing: bool,
}

impl Queue {
    pub fn open(directory: &Path) -> Result<Shared> {
        fs::create_dir_all(directory)?;
        let path = directory.join("downloads.json");
        let jobs = match fs::read(&path) {
            Ok(data) => serde_json::from_slice(&data)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error.into()),
        };
        let mut queue = Self {
            path,
            jobs,
            workers: HashMap::new(),
            closing: false,
        };
        for job in &mut queue.jobs {
            if matches!(job.status.as_str(), "Downloading" | "Queued" | "Pausing") {
                job.status = "Paused".into();
            }
            if let Ok(partial) = fs::metadata(job.sidecar("part")) {
                job.received = partial.len();
                if fs::metadata(&job.destination)
                    .is_ok_and(|dest| dest.ino() == partial.ino() && dest.dev() == partial.dev())
                {
                    job.status = "Complete".into();
                }
            }
            for extension in ["headers", "segment"] {
                let _ = fs::remove_file(job.sidecar(extension));
            }
        }
        queue.save()?;
        for job in &queue.jobs {
            if job.status == "Complete" {
                let _ = fs::remove_file(job.sidecar("part"));
            }
        }
        Ok(Arc::new(Mutex::new(queue)))
    }

    fn save(&self) -> Result<()> {
        let temporary = self.path.with_extension("tmp");
        let file = OpenOptions::new()
            .write(true)
            .truncate(true)
            .create(true)
            .mode(0o600)
            .open(&temporary)?;
        serde_json::to_writer(&file, &self.jobs)?;
        file.sync_all()?;
        fs::rename(temporary, &self.path)?;
        Ok(())
    }

    pub fn add(&mut self, url: &str, directory: &Path) -> Result<()> {
        validate_url(url)?;
        fs::create_dir_all(directory)?;
        let directory = directory.canonicalize()?;
        let raw = url.split("://").nth(1).unwrap_or_default();
        let path = raw.split_once('/').map_or("", |(_, path)| path);
        let candidate = path
            .split(['?', '#'])
            .next()
            .unwrap_or("")
            .rsplit('/')
            .next()
            .unwrap_or("");
        let name: String = candidate
            .chars()
            .filter(|c| !c.is_control() && *c != '\\')
            .take(55)
            .collect();
        let name = if name.trim_matches(['.', ' ']).is_empty() {
            "download"
        } else {
            &name
        };
        let mut destination = directory.join(name);
        let mut index = 1;
        while destination.symlink_metadata().is_ok()
            || self.jobs.iter().any(|job| job.destination == destination)
        {
            destination = directory.join(format!("{index}-{name}"));
            index += 1;
        }
        let id = format!(
            "{:032x}",
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
        );
        self.jobs.push(Job {
            id,
            url: url.into(),
            destination,
            status: "Queued".into(),
            received: 0,
            total: None,
            token: None,
            resource: None,
            error: String::new(),
        });
        if let Err(error) = self.save() {
            self.jobs.pop();
            return Err(error);
        }
        Ok(())
    }

    pub fn action(&mut self, id: &str, action: &str, url: &str) -> Result<()> {
        let job = self
            .jobs
            .iter_mut()
            .find(|job| job.id == id)
            .ok_or("Select a download.")?;
        match (action, job.status.as_str()) {
            ("Pause", "Downloading") => {
                if let Some(cancel) = self.workers.get(id) {
                    cancel.store(true, Ordering::Relaxed);
                }
                job.status = "Pausing".into();
            }
            ("Pause", "Queued") => job.status = "Paused".into(),
            ("Resume" | "Update link" | "Restart", "Paused" | "Failed") => {
                if action == "Update link" {
                    validate_url(url)?;
                    job.url = url.into();
                }
                if action == "Restart" {
                    remove_if_present(&job.sidecar("part"))?;
                    job.received = 0;
                    job.total = None;
                    job.token = None;
                    job.resource = None;
                }
                job.status = "Queued".into();
                job.error.clear();
            }
            ("Remove", "Paused" | "Failed" | "Complete") => {
                remove_if_present(&job.sidecar("part"))?;
                self.jobs.retain(|job| job.id != id);
            }
            _ => return Err("This action is unavailable for the selected transfer.".into()),
        }
        self.save()
    }

    pub fn dispatch(shared: &Shared) {
        let mut queue = shared.lock().unwrap();
        while !queue.closing && queue.workers.len() < 3 {
            let Some(job) = queue.jobs.iter_mut().find(|job| job.status == "Queued") else {
                break;
            };
            job.status = "Downloading".into();
            let job = job.clone();
            let cancel = Arc::new(AtomicBool::new(false));
            queue.workers.insert(job.id.clone(), cancel.clone());
            let shared = shared.clone();
            thread::spawn(move || {
                let outcome = transfer(&shared, &job, &cancel);
                let mut queue = shared.lock().unwrap();
                let row = queue.jobs.iter_mut().find(|row| row.id == job.id).unwrap();
                match outcome {
                    Ok(true) => row.status = "Complete".into(),
                    Ok(false) => row.status = "Paused".into(),
                    Err(error) => {
                        row.status = "Failed".into();
                        row.error = error.to_string();
                    }
                }
                row.received = fs::metadata(row.sidecar("part")).map_or(0, |file| file.len());
                let complete = row.status == "Complete";
                queue.workers.remove(&job.id);
                if queue.save().is_ok() && complete {
                    let _ = fs::remove_file(job.sidecar("part"));
                }
                for extension in ["headers", "segment"] {
                    let _ = fs::remove_file(job.sidecar(extension));
                }
            });
        }
    }

    pub fn close(&mut self) -> bool {
        self.closing = true;
        for cancel in self.workers.values() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.workers.is_empty()
    }
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

fn validate_url(url: &str) -> Result<()> {
    let authority = url
        .split_once("://")
        .filter(|(scheme, _)| matches!(*scheme, "http" | "https"))
        .and_then(|(_, tail)| tail.split(['/', '?', '#']).next());
    if authority.is_none_or(str::is_empty)
        || url.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return Err("Enter an HTTP or HTTPS URL.".into());
    }
    Ok(())
}

struct Response {
    status: u16,
    total: Option<u64>,
    token: Option<String>,
}

fn response(headers: &str, job: &Job, offset: u64, length: u64) -> Result<Response> {
    let block = headers.replace("\r\n", "\n");
    let (status, fields) = block
        .split("\n\n")
        .filter_map(|section| {
            let (line, fields) = section.split_once('\n').unwrap_or((section, ""));
            if !line.starts_with("HTTP/") {
                return None;
            }
            Some((line.split_whitespace().nth(1)?.parse::<u16>().ok()?, fields))
        })
        .last()
        .ok_or("The server did not send an HTTP response.")?;
    let fields: HashMap<_, _> = fields
        .lines()
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.to_ascii_lowercase(), value.trim()))
        .collect();
    let get = |key: &str| fields.get(key).copied();
    if get("content-encoding").is_some_and(|encoding| !encoding.eq_ignore_ascii_case("identity")) {
        return Err("The server returned an unsupported content encoding.".into());
    }
    let token = get("etag")
        .filter(|tag| tag.starts_with('"') && tag.ends_with('"'))
        .or_else(|| {
            let modified = get("last-modified")?;
            let parse = |text: &str| {
                let value = CString::new(text).ok()?;
                let date = unsafe { curl_getdate(value.as_ptr(), std::ptr::null()) };
                (date >= 0).then_some(date)
            };
            (parse(get("date")?)?.saturating_sub(parse(modified)?) >= 60).then_some(modified)
        })
        .map(str::to_owned);
    let size = get("content-length").map(str::parse::<u64>).transpose()?;
    let total = if offset > 0 {
        if status != 206 || token != job.token {
            return Err(format!("The server could not verify the saved bytes (HTTP {status}). Update the link or restart.").into());
        }
        let range = get("content-range")
            .and_then(|value| value.strip_prefix("bytes "))
            .and_then(|value| value.split_once('/'))
            .ok_or("The server returned an invalid byte range.")?;
        let (start, end) = range
            .0
            .split_once('-')
            .ok_or("The server returned an invalid byte range.")?;
        let total = range.1.parse::<u64>()?;
        if start.parse::<u64>()? != offset
            || end.parse::<u64>()?.checked_add(1) != Some(total)
            || total <= offset
            || size.is_some_and(|size| size != total - offset)
            || job.total.is_some_and(|previous| previous != total)
        {
            return Err("The server returned an unexpected byte range.".into());
        }
        Some(total)
    } else {
        if status != 200 {
            return Err(format!("HTTP {status}. Resume or update the link.").into());
        }
        size
    };
    if total.is_some_and(|total| length > total - offset) {
        return Err("The server sent more bytes than expected.".into());
    }
    Ok(Response {
        status,
        total,
        token,
    })
}

fn resource(url: &str) -> serde_json::Value {
    let url = url.split(['?', '#']).next().unwrap_or(url);
    serde_json::Value::String(url.to_owned())
}

fn same_resource(saved: &serde_json::Value, url: &str) -> bool {
    if saved.is_string() {
        return *saved == resource(url);
    }
    // Older Python queues stored the effective URL as four components.
    let Some(parts) = saved.as_array().filter(|parts| parts.len() == 4) else {
        return false;
    };
    let (Some(scheme), Some(host), Some(path)) =
        (parts[0].as_str(), parts[1].as_str(), parts[3].as_str())
    else {
        return false;
    };
    let port = parts[2]
        .as_u64()
        .map_or(String::new(), |port| format!(":{port}"));
    *resource(url).as_str().unwrap() == format!("{scheme}://{host}{port}{path}")
}

fn transfer(shared: &Shared, job: &Job, cancel: &AtomicBool) -> Result<bool> {
    let partial = job.sidecar("part");
    let offset = fs::metadata(&partial).map_or(0, |file| file.len());
    if Some(offset) == job.total && partial.is_file() {
        fs::hard_link(&partial, &job.destination)?;
        return Ok(true);
    }
    if offset > 0 && (job.token.is_none() || job.resource.is_none()) {
        return Err("The server did not provide a resume validator. Restart to download from the beginning.".into());
    }

    let headers = job.sidecar("headers");
    let segment = job.sidecar("segment");
    let url = CString::new(job.url.as_str())?;
    let header_path = CString::new(headers.as_os_str().as_encoded_bytes())?;
    let segment_path = CString::new(segment.as_os_str().as_encoded_bytes())?;
    let range = CString::new(format!("{offset}-"))?;
    let token = job.token.as_deref().unwrap_or_default();
    if token.contains(['\r', '\n']) {
        return Err("Invalid resume validator.".into());
    }
    let validator = CString::new(format!("If-Range: {token}"))?;
    let mut effective = vec![0 as c_char; 131_072];
    let mut progress = Progress {
        shared,
        job,
        cancel,
        offset,
    };
    let exit = unsafe {
        relay_curl(
            url.as_ptr(),
            if offset > 0 {
                range.as_ptr()
            } else {
                std::ptr::null()
            },
            validator.as_ptr(),
            header_path.as_ptr(),
            segment_path.as_ptr(),
            effective.as_mut_ptr(),
            effective.len(),
            transfer_progress,
            (&mut progress as *mut Progress).cast(),
        )
    };
    let effective = unsafe { CStr::from_ptr(effective.as_ptr()) }.to_str()?;
    let paused = cancel.load(Ordering::Relaxed);
    let length = fs::metadata(&segment).map_or(0, |file| file.len());
    let raw_headers = fs::read_to_string(&headers).unwrap_or_default();
    if paused && (raw_headers.is_empty() || length == 0) {
        return Ok(false);
    }
    if raw_headers.is_empty() || effective.is_empty() {
        let message = unsafe { CStr::from_ptr(curl_easy_strerror(exit)) }.to_string_lossy();
        return Err(format!("Connection failed. {message}").into());
    }
    let response = response(&raw_headers, job, offset, length)?;
    if offset > 0 && !same_resource(job.resource.as_ref().unwrap(), &effective) {
        return Err(
            "The replacement points to a different file path. Restart to use that link.".into(),
        );
    }
    {
        let mut queue = shared.lock().unwrap();
        let row = queue.jobs.iter_mut().find(|row| row.id == job.id).unwrap();
        row.token = response.token;
        row.resource = Some(resource(&effective));
        row.total = response.total;
        queue.save()?;
    }
    if offset == 0 && segment.is_file() {
        File::open(&segment)?.sync_all()?;
        fs::rename(&segment, &partial)?;
    } else if segment.is_file() {
        let mut output = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&partial)?;
        io::copy(&mut File::open(&segment)?, &mut output)?;
        output.sync_all()?;
    }
    if paused {
        return Ok(false);
    }
    if exit != 0 || response.total.is_some_and(|total| total != offset + length) {
        return Err("The transfer ended early. Resume to request the remaining bytes.".into());
    }
    if !partial.is_file() && response.status == 200 {
        File::create(&partial)?.sync_all()?;
    }
    fs::hard_link(&partial, &job.destination)?;
    Ok(true)
}

extern "C" {
    pub fn relay_curl_init() -> c_int;
    fn relay_curl(
        url: *const c_char,
        range: *const c_char,
        validator: *const c_char,
        headers: *const c_char,
        segment: *const c_char,
        effective: *mut c_char,
        capacity: usize,
        progress: extern "C" fn(*mut c_void, i64, i64, i64, i64) -> c_int,
        context: *mut c_void,
    ) -> c_int;
    fn curl_easy_strerror(code: c_int) -> *const c_char;
    fn curl_getdate(text: *const c_char, unused: *const std::ffi::c_long) -> std::ffi::c_long;
}

struct Progress<'a> {
    shared: &'a Shared,
    job: &'a Job,
    cancel: &'a AtomicBool,
    offset: u64,
}

extern "C" fn transfer_progress(
    context: *mut c_void,
    _: i64,
    received: i64,
    _: i64,
    _: i64,
) -> c_int {
    let progress = unsafe { &*context.cast::<Progress>() };
    if let Ok(mut queue) = progress.shared.lock() {
        if let Some(row) = queue.jobs.iter_mut().find(|row| row.id == progress.job.id) {
            row.received = progress.offset.saturating_add(received.max(0) as u64);
        }
    }
    progress.cancel.load(Ordering::Relaxed).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved_job() -> Job {
        serde_json::from_str(
            r#"{
            "id":"sample","url":"https://example.test/file?fresh",
            "destination":"/tmp/sample","status":"Paused","received":40,
            "total":100,"token":"\"sample\"","resource":["https","example.test",null,"/file"],
            "error":"","speed":0
        }"#,
        )
        .unwrap()
    }

    #[test]
    fn resumes_legacy_transfer_from_exact_byte_offset() {
        let job = saved_job();
        assert!(same_resource(job.resource.as_ref().unwrap(), &job.url));
        assert!(!same_resource(
            job.resource.as_ref().unwrap(),
            "https://example.test/other"
        ));
        let headers = "HTTP/1.1 206 Partial Content\r\nETag: \"sample\"\r\nContent-Range: bytes 40-99/100\r\nContent-Length: 60\r\n\r\n";
        assert_eq!(response(headers, &job, 40, 60).unwrap().total, Some(100));
        assert!(response(headers, &job, 40, 61).is_err());
        assert!(response(headers, &job, 41, 59).is_err());
        assert!(response(&headers.replace("sample", "changed"), &job, 40, 60).is_err());
        assert!(response(&headers.replace("206", "200"), &job, 40, 60).is_err());
    }

    #[test]
    fn ignores_redirect_headers_and_accepts_a_short_segment_for_resume() {
        let raw = "HTTP/1.1 302 Found\r\nContent-Length: 0\r\n\r\nHTTP/2 200\r\nETag: \"sample\"\r\nContent-Length: 100\r\n\r\n";
        let parsed = response(raw, &saved_job(), 0, 40).unwrap();
        assert_eq!(parsed.total, Some(100));
        assert_eq!(parsed.token.as_deref(), Some("\"sample\""));
    }
}
