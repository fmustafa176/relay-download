mod download;
mod gtk;
mod headless;

use download::{Queue, Shared};
use gtk::*;
use std::{
    env,
    ffi::{c_int, CStr},
    fs::{self, OpenOptions},
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    process::Command,
    ptr::null_mut,
    sync::mpsc::{self, Receiver},
    thread,
};

const REVISION: &str = match option_env!("RELAY_REVISION") {
    Some(revision) => revision,
    None => "development",
};

struct App {
    window: Ptr,
    list: Ptr,
    status: Ptr,
    summary: Ptr,
    empty: Ptr,
    actions: Ptr,
    settings: Ptr,
    rows: Vec<TransferRow>,
    loop_: Ptr,
    queue: Shared,
    state: PathBuf,
    directory: PathBuf,
    ids: Vec<String>,
    controls: Vec<(Ptr, &'static str)>,
    closing: bool,
    notice: String,
    updater: Option<Receiver<String>>,
}

struct TransferRow {
    name: Ptr,
    status: Ptr,
    progress: Ptr,
}

impl TransferRow {
    unsafe fn append(list: Ptr) -> Self {
        let container = gtk_box_new(1, 8);
        let info = gtk_box_new(0, 12);
        let name = gtk_label_new(null_mut());
        gtk_label_set_xalign(name, 0.0);
        gtk_label_set_ellipsize(name, 2);
        gtk_widget_set_hexpand(name, 1);
        gtk_widget_add_css_class(name, string("transfer-name").as_ptr());
        let status = gtk_label_new(null_mut());
        gtk_widget_add_css_class(status, string("muted").as_ptr());
        let progress = gtk_progress_bar_new();
        gtk_widget_add_css_class(progress, string("transfer-progress").as_ptr());
        gtk_box_append(info, name);
        gtk_box_append(info, status);
        gtk_box_append(container, info);
        gtk_box_append(container, progress);
        gtk_list_box_append(list, container);
        Self {
            name,
            status,
            progress,
        }
    }
}

fn display_bytes(bytes: u64) -> String {
    let mut size = bytes as f64;
    for unit in ["B", "KiB", "MiB", "GiB", "TiB"] {
        if size < 1024.0 || unit == "TiB" {
            return if unit == "B" {
                format!("{bytes} B")
            } else {
                format!("{size:.1} {unit}")
            };
        }
        size /= 1024.0;
    }
    unreachable!()
}

struct Dialog {
    app: Ptr,
    source: Ptr,
    folder: Ptr,
    error: Ptr,
    action: &'static str,
    id: Option<String>,
}

impl App {
    unsafe fn refresh(&mut self) {
        if let Some(receiver) = &self.updater {
            match receiver.try_recv() {
                Ok(message) => {
                    self.notice = message;
                    self.updater = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.notice = "Update check could not finish.".into();
                    self.updater = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if self.closing {
            if self.queue.lock().unwrap().close() {
                g_main_loop_quit(self.loop_);
            }
            return;
        }
        Queue::dispatch(&self.queue);
        let jobs = self.queue.lock().unwrap().jobs.clone();
        let selected = self.selected();
        let ids: Vec<_> = jobs.iter().rev().map(|job| job.id.clone()).collect();
        if ids != self.ids {
            loop {
                let row = gtk_list_box_get_row_at_index(self.list, 0);
                if row.is_null() {
                    break;
                }
                gtk_list_box_remove(self.list, row);
            }
            self.rows.clear();
            self.rows
                .extend(ids.iter().map(|_| TransferRow::append(self.list)));
            if let Some(index) = selected.and_then(|id| ids.iter().position(|item| item == &id)) {
                gtk_list_box_select_row(
                    self.list,
                    gtk_list_box_get_row_at_index(self.list, index as c_int),
                );
            }
            self.ids = ids;
        }
        gtk_widget_set_visible(self.empty, jobs.is_empty().into());
        for (row, job) in self.rows.iter().zip(jobs.iter().rev()) {
            let filename = job
                .destination
                .file_name()
                .unwrap_or_default()
                .to_string_lossy();
            label(row.name, &filename);
            let amount = display_bytes(job.received);
            let amount = job.total.map_or(amount.clone(), |total| {
                format!("{amount} / {}", display_bytes(total))
            });
            label(row.status, &format!("{}  ·  {amount}", job.status));
            match (job.status.as_str(), job.total) {
                ("Complete", _) => gtk_progress_bar_set_fraction(row.progress, 1.0),
                (_, Some(total)) if total > 0 => {
                    gtk_progress_bar_set_fraction(
                        row.progress,
                        (job.received as f64 / total as f64).min(1.0),
                    );
                }
                ("Downloading", None) => gtk_progress_bar_pulse(row.progress),
                _ => gtk_progress_bar_set_fraction(row.progress, 0.0),
            }
        }
        let active = jobs
            .iter()
            .filter(|job| matches!(job.status.as_str(), "Downloading" | "Queued" | "Pausing"))
            .count();
        let count = jobs.len();
        let noun = if count == 1 { "transfer" } else { "transfers" };
        label(self.summary, &format!("{count} {noun}  ·  {active} active"));
        let selected = self.selected();
        let job = jobs.iter().find(|job| Some(&job.id) == selected.as_ref());
        let status = job.map_or("", |job| job.status.as_str());
        let mut show_actions = false;
        for (widget, action) in &self.controls {
            if matches!(*action, "Check updates" | "Update now") {
                gtk_widget_set_sensitive(*widget, self.updater.is_none().into());
                continue;
            }
            let available = match *action {
                "Pause" => matches!(status, "Queued" | "Downloading"),
                "Remove" => matches!(status, "Paused" | "Failed" | "Complete"),
                _ => matches!(status, "Paused" | "Failed"),
            };
            gtk_widget_set_visible(*widget, available.into());
            show_actions |= available;
        }
        gtk_widget_set_visible(self.actions, show_actions.into());
        let message = if let Some(job) = job
            .filter(|job| !job.error.is_empty() && self.updater.is_none() && self.notice.is_empty())
        {
            &job.error
        } else {
            &self.notice
        };
        label(self.status, message);
        let tooltip = string(message);
        gtk_widget_set_tooltip_text(
            self.status,
            if message.is_empty() {
                std::ptr::null()
            } else {
                tooltip.as_ptr()
            },
        );
    }

    unsafe fn selected(&self) -> Option<String> {
        let row = gtk_list_box_get_selected_row(self.list);
        if row.is_null() {
            return None;
        }
        self.ids
            .get(gtk_list_box_row_get_index(row) as usize)
            .cloned()
    }

    unsafe fn dialog(&mut self, action: &'static str, initial: &str) {
        let id = self.selected();
        let dialog = gtk_dialog_new();
        gtk_widget_add_css_class(dialog, string("relay-dialog").as_ptr());
        gtk_window_set_title(dialog, string(action).as_ptr());
        gtk_window_set_default_size(dialog, 500, -1);
        gtk_window_set_transient_for(dialog, self.window);
        gtk_window_set_modal(dialog, 1);
        let area = gtk_dialog_get_content_area(dialog);
        let fields = gtk_box_new(1, 10);
        gtk_widget_add_css_class(fields, string("dialog-content").as_ptr());
        gtk_box_append(area, fields);
        let mut source = null_mut();
        let mut folder = null_mut();
        match action {
            "Add download" | "Update link" | "Torrents" => {
                source = gtk_entry_new();
                let hint = if action == "Torrents" {
                    "Magnet, torrent URL, or absolute .torrent path"
                } else {
                    "HTTP or HTTPS URL"
                };
                gtk_entry_set_placeholder_text(source, string(hint).as_ptr());
                gtk_editable_set_text(source, string(initial).as_ptr());
                gtk_entry_set_activates_default(source, 1);
                gtk_box_append(fields, source);
                if action == "Add download" {
                    folder = gtk_entry_new();
                    gtk_entry_set_placeholder_text(folder, string("Destination folder").as_ptr());
                    gtk_editable_set_text(
                        folder,
                        string(&self.directory.to_string_lossy()).as_ptr(),
                    );
                    gtk_entry_set_activates_default(folder, 1);
                    gtk_box_append(fields, folder);
                } else if action == "Torrents" {
                    gtk_dialog_add_button(dialog, string("Open qBittorrent").as_ptr(), 2);
                }
            }
            "Restart" | "Remove" => {
                let prompt = if action == "Restart" {
                    "Discard the partial download and start over?"
                } else {
                    "Remove this transfer and its partial file? Completed files are kept."
                };
                gtk_box_append(fields, gtk_label_new(string(prompt).as_ptr()));
            }
            _ => {}
        }
        let error = gtk_label_new(null_mut());
        gtk_label_set_xalign(error, 0.0);
        gtk_label_set_wrap(error, 1);
        gtk_widget_add_css_class(error, string("field-error").as_ptr());
        gtk_widget_set_visible(error, 0);
        gtk_box_append(fields, error);
        let cancel = gtk_dialog_add_button(dialog, string("Cancel").as_ptr(), -6);
        gtk_widget_add_css_class(cancel, string("secondary").as_ptr());
        let confirm = gtk_dialog_add_button(
            dialog,
            string(if action == "Torrents" {
                "Add torrent"
            } else {
                action
            })
            .as_ptr(),
            1,
        );
        let style = if matches!(action, "Restart" | "Remove") {
            "danger"
        } else {
            "primary"
        };
        gtk_widget_add_css_class(confirm, string(style).as_ptr());
        if !matches!(action, "Restart" | "Remove") {
            gtk_dialog_set_default_response(dialog, 1);
        }
        let context = Box::new(Dialog {
            app: (self as *mut Self).cast(),
            source,
            folder,
            error,
            action,
            id,
        });
        connect(
            dialog,
            "response",
            response as Ptr,
            Box::into_raw(context).cast(),
        );
        gtk_window_present(dialog);
    }

    fn torrent(&self, source: &str) -> std::io::Result<()> {
        let mut command = Command::new("qbittorrent");
        command
            .arg(format!(
                "--profile={}",
                self.state.join("torrent-profile").display()
            ))
            .arg("--no-splash");
        if !source.is_empty() {
            if !source.starts_with("magnet:")
                && !source.starts_with("http://")
                && !source.starts_with("https://")
                && !Path::new(source).is_absolute()
            {
                return Err(std::io::Error::other(
                    "Enter a magnet link, torrent URL, or absolute .torrent path.",
                ));
            }
            command.arg(source);
        }
        let mut process = command.spawn()?;
        thread::spawn(move || {
            let _ = process.wait();
        });
        Ok(())
    }

    fn check_updates(&mut self, update: bool) {
        if self.updater.is_some() {
            return;
        }
        let helper = env::current_exe()
            .ok()
            .and_then(|path| Some(path.parent()?.parent()?.join("lib/relay/update")));
        let Some(helper) = helper.filter(|path| path.is_file()) else {
            self.notice = "Install Relay to enable GitHub updates.".into();
            return;
        };
        let (sender, receiver) = mpsc::channel();
        self.updater = Some(receiver);
        self.notice = if update {
            "Building the GitHub update…"
        } else {
            "Checking GitHub…"
        }
        .into();
        thread::spawn(move || {
            let message = match Command::new(helper)
                .arg(if update { "--apply" } else { "--check" })
                .output()
            {
                Ok(output) if output.status.success() => {
                    String::from_utf8_lossy(&output.stdout).trim().to_owned()
                }
                Ok(output) => {
                    let error = String::from_utf8_lossy(&output.stderr);
                    error
                        .lines()
                        .last()
                        .unwrap_or("GitHub update failed.")
                        .to_owned()
                }
                Err(error) => error.to_string(),
            };
            let _ = sender.send(message);
        });
    }
}

extern "C" fn response(dialog: Ptr, result: c_int, data: Ptr) {
    unsafe {
        let context = &*data.cast::<Dialog>();
        let app = &mut *context.app.cast::<App>();
        if result == 1 || (result == 2 && context.action == "Torrents") {
            let source = if context.source.is_null() {
                String::new()
            } else {
                text(context.source).trim().to_owned()
            };
            app.notice.clear();
            let outcome: Result<(), Box<dyn std::error::Error>> = match context.action {
                "Torrents" if result == 1 && source.is_empty() => {
                    Err("Enter a magnet link, torrent URL, or .torrent path.".into())
                }
                "Torrents" => app
                    .torrent(if result == 2 { "" } else { &source })
                    .map_err(Into::into),
                "Add download" => {
                    let folder = text(context.folder);
                    if folder.trim().is_empty() {
                        Err("Enter a destination folder.".into())
                    } else {
                        let directory = PathBuf::from(folder.trim());
                        let result = app.queue.lock().unwrap().add(&source, &directory);
                        if result.is_ok() {
                            app.directory = directory;
                        }
                        result
                    }
                }
                action => app.queue.lock().unwrap().action(
                    context.id.as_deref().unwrap_or(""),
                    action,
                    &source,
                ),
            };
            if let Err(error) = outcome {
                label(context.error, &error.to_string());
                gtk_widget_set_visible(context.error, 1);
                return;
            }
        }
        gtk_window_destroy(dialog);
        drop(Box::from_raw(data.cast::<Dialog>()));
    }
}

unsafe fn application(data: Ptr) -> &'static mut App {
    &mut *data.cast::<App>()
}
extern "C" fn tick(data: Ptr) -> Bool {
    unsafe {
        application(data).refresh();
    }
    1
}
extern "C" fn add(_: Ptr, data: Ptr) {
    unsafe {
        application(data).dialog("Add download", "");
    }
}
extern "C" fn torrents(_: Ptr, data: Ptr) {
    unsafe {
        application(data).dialog("Torrents", "");
    }
}
extern "C" fn check(_: Ptr, data: Ptr) {
    unsafe {
        let app = application(data);
        gtk_menu_button_popdown(app.settings);
        app.check_updates(false);
    }
}
extern "C" fn update(_: Ptr, data: Ptr) {
    unsafe {
        let app = application(data);
        gtk_menu_button_popdown(app.settings);
        app.check_updates(true);
    }
}
extern "C" fn minimize(_: Ptr, data: Ptr) {
    unsafe {
        gtk_window_minimize(application(data).window);
    }
}
extern "C" fn maximize(_: Ptr, data: Ptr) {
    unsafe {
        let window = application(data).window;
        if gtk_window_is_maximized(window) != 0 {
            gtk_window_unmaximize(window);
        } else {
            gtk_window_maximize(window);
        }
    }
}
extern "C" fn close(_: Ptr, data: Ptr) {
    unsafe {
        gtk_window_close(application(data).window);
    }
}
extern "C" fn closing(_: Ptr, data: Ptr) -> Bool {
    unsafe {
        let app = application(data);
        app.closing = true;
        label(app.status, "Saving download progress…");
    }
    1
}
unsafe fn action(data: Ptr, action: &'static str) {
    let app = application(data);
    let Some(id) = app.selected() else { return };
    app.notice.clear();
    if matches!(action, "Pause" | "Resume") {
        if let Err(error) = app.queue.lock().unwrap().action(&id, action, "") {
            app.notice = error.to_string();
        }
    } else {
        let initial = app
            .queue
            .lock()
            .unwrap()
            .jobs
            .iter()
            .find(|job| job.id == id)
            .map(|job| job.url.clone())
            .unwrap_or_default();
        app.dialog(action, &initial);
    }
}
extern "C" fn pause(_: Ptr, data: Ptr) {
    unsafe {
        action(data, "Pause");
    }
}
extern "C" fn resume(_: Ptr, data: Ptr) {
    unsafe {
        action(data, "Resume");
    }
}
extern "C" fn replace(_: Ptr, data: Ptr) {
    unsafe {
        action(data, "Update link");
    }
}
extern "C" fn restart(_: Ptr, data: Ptr) {
    unsafe {
        action(data, "Restart");
    }
}
extern "C" fn remove(_: Ptr, data: Ptr) {
    unsafe {
        action(data, "Remove");
    }
}

extern "C" {
    fn flock(fd: c_int, operation: c_int) -> c_int;
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<_> = env::args().skip(1).collect();
    let source = arguments.first().cloned();
    let is_headless = source.as_deref() == Some("--headless");
    if source.as_deref() == Some("--version") {
        println!("Relay {} ({REVISION})", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if source.as_deref() == Some("--help") {
        println!(
            "Usage: relay [HTTP URL | magnet link | .torrent file]\n\n{}",
            headless::HELP
        );
        return Ok(());
    }
    let home = PathBuf::from(env::var_os("HOME").ok_or("HOME is unavailable.")?);
    let state = env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share"))
        .join("relay");
    if is_headless {
        match &arguments[1..] {
            [] => {
                println!("{}", headless::HELP);
                return Ok(());
            }
            [command] if command == "--help" => {
                println!("{}", headless::HELP);
                return Ok(());
            }
            [command, rest @ ..] if command == "torrents" && rest.len() <= 1 => {
                fs::create_dir_all(&state)?;
                return Err(headless::torrents(&state, rest.first().map(String::as_str)).into());
            }
            _ => {}
        }
    }
    fs::create_dir_all(&state)?;
    let lock = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(state.join("rust.lock"))?;
    if unsafe { flock(lock.as_raw_fd(), 6) } != 0 {
        return Err("Relay is already running.".into());
    }
    // The legacy Qt version holds this lock while it has the same queue open.
    if state.join("relay.lock").is_file() {
        let contents = fs::read_to_string(state.join("relay.lock"))?;
        if let Some(pid) = contents
            .lines()
            .next()
            .and_then(|line| line.parse::<u32>().ok())
        {
            if Path::new("/proc").join(pid.to_string()).exists() {
                return Err("Close the previous version of Relay first.".into());
            }
        }
    }
    let queue = Queue::open(&state)?;
    let directory = download_directory(&home);
    unsafe {
        if relay_init_failed() {
            return Err("Relay could not initialize libcurl.".into());
        }
        if is_headless {
            return headless::run(&queue, &arguments[1..], directory);
        }
        if gtk_init_check() == 0 {
            return Err("Relay could not open a graphical display.".into());
        }
        let css = gtk_css_provider_new();
        gtk_css_provider_load_from_data(
            css,
            include_str!("style.css").as_ptr().cast(),
            include_str!("style.css").len() as isize,
        );
        gtk_style_context_add_provider_for_display(gdk_display_get_default(), css, 800);
        g_object_unref(css);
        let xml = include_str!("ui.xml");
        let builder = gtk_builder_new_from_string(xml.as_ptr().cast(), xml.len() as isize);
        if builder.is_null() {
            return Err("Relay could not load its interface.".into());
        }
        let window = object(builder, "window");
        let loop_ = g_main_loop_new(null_mut(), 0);
        let controls = [
            ("pause", "Pause"),
            ("resume", "Resume"),
            ("replace", "Update link"),
            ("restart", "Restart"),
            ("remove", "Remove"),
            ("check", "Check updates"),
            ("update", "Update now"),
        ]
        .into_iter()
        .map(|(id, action)| (object(builder, id), action))
        .collect();
        let mut app = Box::new(App {
            window,
            list: object(builder, "list"),
            status: object(builder, "status"),
            summary: object(builder, "summary"),
            empty: object(builder, "empty"),
            actions: object(builder, "actions"),
            settings: object(builder, "settings"),
            rows: Vec::new(),
            loop_,
            queue,
            state,
            directory,
            ids: Vec::new(),
            controls,
            closing: false,
            notice: String::new(),
            updater: None,
        });
        let data = (&mut *app as *mut App).cast();
        for (id, callback) in [
            ("add", add as Ptr),
            ("torrents", torrents as Ptr),
            ("min", minimize as Ptr),
            ("max", maximize as Ptr),
            ("close", close as Ptr),
            ("pause", pause as Ptr),
            ("resume", resume as Ptr),
            ("replace", replace as Ptr),
            ("restart", restart as Ptr),
            ("remove", remove as Ptr),
            ("check", check as Ptr),
            ("update", update as Ptr),
        ] {
            connect(object(builder, id), "clicked", callback, data);
        }
        connect(window, "close-request", closing as Ptr, data);
        gtk_window_present(window);
        if env::var_os("RELAY_UI_SMOKE").is_some() {
            app.closing = true;
        } else {
            if env::current_exe()
                .ok()
                .and_then(|path| Some(path.parent()?.parent()?.join("lib/relay/update")))
                .is_some_and(|path| path.is_file())
            {
                app.check_updates(false);
            }
            if let Some(source) = source {
                if source.starts_with("magnet:") || source.ends_with(".torrent") {
                    let path = Path::new(&source);
                    let result = if path.is_file() {
                        app.torrent(&path.canonicalize()?.to_string_lossy())
                    } else {
                        app.torrent(&source)
                    };
                    if let Err(error) = result {
                        app.notice = error.to_string();
                    }
                } else {
                    app.dialog("Add download", &source);
                }
            }
        }
        g_timeout_add(150, tick, data);
        g_main_loop_run(loop_);
        gtk_window_destroy(window);
        g_object_unref(builder);
    }
    Ok(())
}

fn download_directory(home: &Path) -> PathBuf {
    let downloads = unsafe { g_get_user_special_dir(2) };
    if downloads.is_null() {
        home.join("Downloads")
    } else {
        PathBuf::from(
            unsafe { CStr::from_ptr(downloads) }
                .to_string_lossy()
                .as_ref(),
        )
    }
}

unsafe fn relay_init_failed() -> bool {
    download::relay_curl_init() != 0
}

fn main() {
    if let Err(error) = run() {
        eprintln!("Relay: {error}");
        std::process::exit(1);
    }
}
