use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};
pub type Ptr = *mut c_void;
pub type Bool = c_int;

pub fn string(text: &str) -> CString {
    CString::new(text.replace('\0', "�")).unwrap()
}

pub unsafe fn object(builder: Ptr, name: &str) -> Ptr {
    let ptr = gtk_builder_get_object(builder, string(name).as_ptr());
    assert!(!ptr.is_null(), "Missing interface object {name}");
    ptr
}

pub unsafe fn connect(widget: Ptr, signal: &str, callback: Ptr, data: Ptr) {
    g_signal_connect_data(widget, string(signal).as_ptr(), callback, data, None, 0);
}

pub unsafe fn text(entry: Ptr) -> String {
    CStr::from_ptr(gtk_editable_get_text(entry))
        .to_string_lossy()
        .into_owned()
}

pub unsafe fn label(widget: Ptr, text: &str) {
    gtk_label_set_text(widget, string(text).as_ptr());
}

#[link(name = "gtk-4")]
#[link(name = "gobject-2.0")]
#[link(name = "glib-2.0")]
extern "C" {
    pub fn gtk_init_check() -> Bool;
    pub fn gtk_builder_new_from_string(text: *const c_char, length: isize) -> Ptr;
    fn gtk_builder_get_object(builder: Ptr, name: *const c_char) -> Ptr;
    pub fn gtk_css_provider_new() -> Ptr;
    pub fn gtk_css_provider_load_from_data(provider: Ptr, text: *const c_char, length: isize);
    pub fn gtk_style_context_add_provider_for_display(
        display: Ptr,
        provider: Ptr,
        priority: c_uint,
    );
    pub fn gdk_display_get_default() -> Ptr;
    pub fn gtk_window_present(window: Ptr);
    pub fn gtk_window_close(window: Ptr);
    pub fn gtk_window_minimize(window: Ptr);
    pub fn gtk_window_maximize(window: Ptr);
    pub fn gtk_window_unmaximize(window: Ptr);
    pub fn gtk_window_is_maximized(window: Ptr) -> Bool;
    pub fn gtk_window_destroy(window: Ptr);
    pub fn gtk_window_set_title(window: Ptr, title: *const c_char);
    pub fn gtk_window_set_transient_for(window: Ptr, parent: Ptr);
    pub fn gtk_window_set_modal(window: Ptr, modal: Bool);
    pub fn gtk_window_set_default_size(window: Ptr, width: c_int, height: c_int);
    pub fn gtk_label_new(text: *const c_char) -> Ptr;
    fn gtk_label_set_text(widget: Ptr, text: *const c_char);
    pub fn gtk_label_set_xalign(widget: Ptr, alignment: f32);
    pub fn gtk_list_box_append(list: Ptr, child: Ptr);
    pub fn gtk_list_box_get_row_at_index(list: Ptr, index: c_int) -> Ptr;
    pub fn gtk_list_box_select_row(list: Ptr, row: Ptr);
    pub fn gtk_list_box_remove(list: Ptr, child: Ptr);
    pub fn gtk_list_box_get_selected_row(list: Ptr) -> Ptr;
    pub fn gtk_list_box_row_get_index(row: Ptr) -> c_int;
    pub fn gtk_list_box_row_get_child(row: Ptr) -> Ptr;
    pub fn gtk_widget_set_sensitive(widget: Ptr, sensitive: Bool);
    pub fn gtk_widget_add_css_class(widget: Ptr, class: *const c_char);
    pub fn gtk_dialog_new() -> Ptr;
    pub fn gtk_dialog_get_content_area(dialog: Ptr) -> Ptr;
    pub fn gtk_dialog_add_button(dialog: Ptr, label: *const c_char, response: c_int) -> Ptr;
    pub fn gtk_box_append(container: Ptr, child: Ptr);
    pub fn gtk_entry_new() -> Ptr;
    pub fn gtk_entry_set_placeholder_text(entry: Ptr, text: *const c_char);
    pub fn gtk_editable_set_text(entry: Ptr, text: *const c_char);
    fn gtk_editable_get_text(entry: Ptr) -> *const c_char;
    pub fn g_main_loop_new(context: Ptr, running: Bool) -> Ptr;
    pub fn g_main_loop_run(loop_: Ptr);
    pub fn g_main_loop_quit(loop_: Ptr);
    pub fn g_timeout_add(
        interval: c_uint,
        callback: extern "C" fn(Ptr) -> Bool,
        data: Ptr,
    ) -> c_uint;
    pub fn g_object_unref(object: Ptr);
    fn g_signal_connect_data(
        widget: Ptr,
        signal: *const c_char,
        callback: Ptr,
        data: Ptr,
        destroy: Option<extern "C" fn(Ptr, Ptr)>,
        flags: c_uint,
    ) -> u64;
    pub fn g_get_user_special_dir(directory: c_int) -> *const c_char;
}
