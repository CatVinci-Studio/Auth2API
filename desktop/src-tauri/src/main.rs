// Keeps a console window from appearing alongside the app on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    auth2api_desktop_lib::run()
}
