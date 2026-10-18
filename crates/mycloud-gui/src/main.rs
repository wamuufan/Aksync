mod app;
mod i18n;
mod setup_wizard;
mod tauri_bridge;

use app::App;
use leptos::*;

fn main() {
    console_error_panic_hook::set_once();
    mount_to_body(|| view! { <App/> });
}
