use crate::i18n::Language;
use crate::setup_wizard::SetupWizard;
use crate::tauri_bridge::{self, DaemonStatusPayload, SyncFolderInfo, SyncState, SystemThemeInfo, TransferStats};
use leptos::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActiveTab {
    SyncFolders,
    VaultSecurity,
    AppSettings,
}

fn render_folder_icon(icon: &str) -> impl IntoView {
    match icon {
        "desktop" => view! {
            <svg width="22" height="22" style="width: 1.45rem; height: 1.45rem;" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <rect width="20" height="14" x="2" y="3" rx="2"/><line x1="8" x2="16" y1="21" y2="21"/><line x1="12" x2="12" y1="17" y2="21"/>
            </svg>
        }.into_view(),
        "documents" => view! {
            <svg width="22" height="22" style="width: 1.45rem; height: 1.45rem;" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <path d="M14.5 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7.5L14.5 2z"/><polyline points="14 2 14 8 20 8"/><line x1="16" x2="8" y1="13" y2="13"/><line x1="16" x2="8" y1="17" y2="17"/><line x1="10" x2="8" y1="9" y2="9"/>
            </svg>
        }.into_view(),
        "downloads" => view! {
            <svg width="22" height="22" style="width: 1.45rem; height: 1.45rem;" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/><line x1="12" x2="12" y1="15" y2="3"/>
            </svg>
        }.into_view(),
        "music" => view! {
            <svg width="22" height="22" style="width: 1.45rem; height: 1.45rem;" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/>
            </svg>
        }.into_view(),
        "pictures" => view! {
            <svg width="22" height="22" style="width: 1.45rem; height: 1.45rem;" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <rect width="18" height="18" x="3" y="3" rx="2" ry="2"/><circle cx="9" cy="9" r="2"/><path d="m21 15-3.086-3.086a2 2 0 0 0-2.828 0L6 21"/>
            </svg>
        }.into_view(),
        "videos" => view! {
            <svg width="22" height="22" style="width: 1.45rem; height: 1.45rem;" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <rect width="18" height="14" x="3" y="5" rx="2"/><polyline points="10 9 15 12 10 15 10 9"/>
            </svg>
        }.into_view(),
        _ => view! {
            <svg width="22" height="22" style="width: 1.45rem; height: 1.45rem;" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/>
            </svg>
        }.into_view(),
    }
}

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.0} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

fn format_speed(bytes_per_sec: u64) -> String {
    format!("{}/s", format_bytes(bytes_per_sec))
}

fn to_valid_hex_7(s: &str, fallback: &str) -> String {
    let s = s.trim();
    if s.starts_with('#') && s.len() == 7 && s[1..].chars().all(|c| c.is_ascii_hexdigit()) {
        s.to_string()
    } else if s.starts_with('#') && s.len() == 4 && s[1..].chars().all(|c| c.is_ascii_hexdigit()) {
        let chars: Vec<char> = s[1..].chars().collect();
        format!("#{0}{0}{1}{1}{2}{2}", chars[0], chars[1], chars[2])
    } else {
        fallback.to_string()
    }
}

fn is_color_dark(hex: &str) -> bool {
    let clean = hex.trim().trim_start_matches('#');
    if clean.len() == 6 {
        let r = u8::from_str_radix(&clean[0..2], 16).unwrap_or(0) as f64;
        let g = u8::from_str_radix(&clean[2..4], 16).unwrap_or(0) as f64;
        let b = u8::from_str_radix(&clean[4..6], 16).unwrap_or(0) as f64;
        (0.299 * r + 0.587 * g + 0.114 * b) < 140.0
    } else {
        true
    }
}

fn get_storage_item(key: &str) -> Option<String> {
    let win = leptos::web_sys::window()?;
    let storage = js_sys::Reflect::get(&win, &wasm_bindgen::JsValue::from_str("localStorage")).ok()?;
    let val = js_sys::Reflect::get(&storage, &wasm_bindgen::JsValue::from_str(key)).ok()?;
    val.as_string()
}

fn set_storage_item(key: &str, val: &str) {
    if let Some(win) = leptos::web_sys::window() {
        if let Ok(storage) = js_sys::Reflect::get(&win, &wasm_bindgen::JsValue::from_str("localStorage")) {
            let _ = js_sys::Reflect::set(
                &storage,
                &wasm_bindgen::JsValue::from_str(key),
                &wasm_bindgen::JsValue::from_str(val),
            );
        }
    }
}

#[component]
pub fn App() -> impl IntoView {
    let (lang, set_lang) = create_signal(Language::Turkish);
    let (active_tab, set_active_tab) = create_signal(ActiveTab::SyncFolders);
    let (is_initialized, set_is_initialized) = create_signal(false);
    let (is_setup_done, set_is_setup_done) = create_signal(false);

    // Appearance & Settings signals
    let (theme_mode, set_theme_mode) = create_signal(
        get_storage_item("aksync_theme_mode").unwrap_or_else(|| "system".to_string())
    );
    let initial_scale = get_storage_item("aksync_ui_scale").unwrap_or_else(|| "100".to_string());
    let (ui_scale, set_ui_scale) = create_signal(initial_scale.clone());
    let (sys_theme_info, set_sys_theme_info) = create_signal(SystemThemeInfo {
        desktop_env: "Detecting...".to_string(),
        theme_name: "Detecting...".to_string(),
        is_dark: true,
        accent_color: "#3daee9".to_string(),
        window_bg: "#232629".to_string(),
        view_bg: "#1b1e20".to_string(),
        card_bg: "#31363b".to_string(),
        text_primary: "#eff0f1".to_string(),
        text_secondary: "#bdc3c7".to_string(),
        border_color: "rgba(255, 255, 255, 0.12)".to_string(),
        is_fallback: false,
    });
    let (custom_theme_enabled, set_custom_theme_enabled) = create_signal(
        get_storage_item("aksync_custom_theme_enabled").as_deref() == Some("true")
    );
    let (custom_window_bg, set_custom_window_bg) = create_signal(
        get_storage_item("aksync_custom_window_bg").unwrap_or_else(|| "#232629".to_string())
    );
    let (custom_card_bg, set_custom_card_bg) = create_signal(
        get_storage_item("aksync_custom_card_bg").unwrap_or_else(|| "#31363b".to_string())
    );
    let (custom_accent_color, set_custom_accent_color) = create_signal(
        get_storage_item("aksync_custom_accent_color").unwrap_or_else(|| "#3daee9".to_string())
    );
    let (custom_text_color, set_custom_text_color) = create_signal(
        get_storage_item("aksync_custom_text_color").unwrap_or_else(|| "#eff0f1".to_string())
    );

    let apply_ui_scale_to_dom = |scale_str: &str| {
        let (factor, font_size_px) = match scale_str {
            "90" => ("0.9", "13.5px"),
            "100" => ("1.0", "15px"),
            "110" => ("1.1", "16.5px"),
            "125" => ("1.25", "18.75px"),
            "150" => ("1.5", "22.5px"),
            _ => ("1.0", "15px"),
        };
        if let Some(win) = leptos::web_sys::window() {
            if let Some(doc) = win.document() {
                if let Some(root_el) = doc.document_element() {
                    let _ = root_el.set_attribute(
                        "style",
                        &format!("--ui-scale: {}; font-size: {};", factor, font_size_px),
                    );
                    if let Some(html_el) = wasm_bindgen::JsCast::dyn_ref::<leptos::web_sys::HtmlElement>(&root_el) {
                        let style = html_el.style();
                        let _ = style.set_property("--ui-scale", factor);
                        let _ = style.set_property("font-size", font_size_px);
                    }
                }
                if let Some(body_el) = doc.body() {
                    let style = body_el.style();
                    let _ = style.set_property("--ui-scale", factor);
                }
            }
        }
    };

    apply_ui_scale_to_dom(&initial_scale);

    create_effect(move |_| {
        let current = ui_scale.get();
        apply_ui_scale_to_dom(&current);
    });

    let select_ui_scale = move |scale: &'static str| {
        set_ui_scale.set(scale.to_string());
        set_storage_item("aksync_ui_scale", scale);
        apply_ui_scale_to_dom(scale);
    };

    // Keyboard shortcuts listener (Ctrl + / Ctrl - / Ctrl 0)
    create_effect(move |_| {
        if let Some(win) = leptos::web_sys::window() {
            let cb = wasm_bindgen::closure::Closure::<dyn Fn(web_sys::KeyboardEvent)>::new(move |ev: web_sys::KeyboardEvent| {
                if ev.ctrl_key() || ev.meta_key() {
                    let key = ev.key();
                    let steps = ["90", "100", "110", "125", "150"];
                    let cur = ui_scale.get_untracked();
                    let idx = steps.iter().position(|&s| s == cur).unwrap_or(1);
                    if key == "+" || key == "=" {
                        ev.prevent_default();
                        let next_idx = (idx + 1).min(steps.len() - 1);
                        let next = steps[next_idx];
                        set_ui_scale.set(next.to_string());
                        set_storage_item("aksync_ui_scale", next);
                        apply_ui_scale_to_dom(next);
                    } else if key == "-" || key == "_" {
                        ev.prevent_default();
                        let next_idx = idx.saturating_sub(1);
                        let next = steps[next_idx];
                        set_ui_scale.set(next.to_string());
                        set_storage_item("aksync_ui_scale", next);
                        apply_ui_scale_to_dom(next);
                    } else if key == "0" {
                        ev.prevent_default();
                        set_ui_scale.set("100".to_string());
                        set_storage_item("aksync_ui_scale", "100");
                        apply_ui_scale_to_dom("100");
                    }
                }
            });
            let _ = win.add_event_listener_with_callback("keydown", wasm_bindgen::JsCast::unchecked_ref(cb.as_ref()));
            cb.forget();
        }
    });

    // Setup wizard signals lifted here so re-renders never wipe inputs or focus
    let (step1_done, set_step1_done) = create_signal(false);
    let (password, set_password) = create_signal(String::new());
    let (confirm_password, set_confirm_password) = create_signal(String::new());

    let (status, set_status) = create_signal(DaemonStatusPayload {
        state: SyncState::Locked,
        rclone_pid: None,
        fuse_pid: None,
        raw_mount_path: "~/.cache/mycloud/Google Drive".to_string(),
        target_mount_path: "~/MyCloud".to_string(),
    });
    let (stats, set_stats) = create_signal(TransferStats {
        upload_speed: 0,
        download_speed: 0,
        storage_used: 0,
        storage_total: 0,
        files_synced: 0,
    });
    let (is_paused, set_is_paused) = create_signal(false);
    let (is_syncing_anim, set_is_syncing_anim) = create_signal(false);
    let (sync_folders, set_sync_folders) = create_signal(Vec::<SyncFolderInfo>::new());
    let (sync_hidden_files, set_sync_hidden_files) = create_signal(false);
    let password_input_ref = create_node_ref::<html::Input>();

    // Custom Google Drive API credentials
    let (custom_client_id, set_custom_client_id) = create_signal(String::new());
    let (custom_client_secret, set_custom_client_secret) = create_signal(String::new());
    let (api_saving, set_api_saving) = create_signal(false);
    let (api_authorizing, set_api_authorizing) = create_signal(false);
    let (api_feedback_msg, set_api_feedback_msg) = create_signal(Option::<(bool, String)>::None);

    // Single unified initialization and polling sequence (prevents any visual view flickers)
    create_effect(move |_| {
        leptos::spawn_local(async move {
            // Step 0: Query system theme & accent color from Linux desktop
            let sys_info = tauri_bridge::get_system_theme_info().await;
            set_sys_theme_info.set(sys_info);

            // Step 1: Initial atomic fetch
            let st = tauri_bridge::get_sync_state().await;
            let paused = matches!(st.state, SyncState::Paused);
            set_is_paused.set(paused);

            if matches!(st.state, SyncState::SetupRequired) {
                if let Ok(setup_info) = tauri_bridge::check_setup_status().await {
                    if setup_info.rclone_configured {
                        set_step1_done.set(true);
                    }
                }
                set_is_setup_done.set(false);
            } else {
                set_is_setup_done.set(true);
            }

            set_status.set(st);

            let tr = tauri_bridge::get_transfer_stats().await;
            set_stats.set(tr);

            let flds = tauri_bridge::get_sync_folders().await;
            set_sync_folders.set(flds);

            set_sync_hidden_files.set(false);
            let _ = tauri_bridge::set_sync_hidden_files(false).await;

            if let Ok(creds) = tauri_bridge::get_drive_api_credentials().await {
                set_custom_client_id.set(creds.client_id);
                set_custom_client_secret.set(creds.client_secret);
            }

            // Mark ready: exact single transition directly to the designated screen
            set_is_initialized.set(true);

            // Step 2: Periodic fast background polling (600ms)
            loop {
                gloo_timers::future::TimeoutFuture::new(600).await;

                // Dynamically sync system theme changes (KDE Plasma / GTK)
                let fresh_theme = tauri_bridge::get_system_theme_info().await;
                if sys_theme_info.get_untracked() != fresh_theme {
                    set_sys_theme_info.set(fresh_theme);
                }

                let st = tauri_bridge::get_sync_state().await;
                let paused = matches!(st.state, SyncState::Paused);
                if is_paused.get_untracked() != paused {
                    set_is_paused.set(paused);
                }

                if !matches!(st.state, SyncState::SetupRequired | SyncState::Offline)
                    && !is_setup_done.get_untracked()
                {
                    set_is_setup_done.set(true);
                }

                set_status.set(st);

                let tr = tauri_bridge::get_transfer_stats().await;
                set_stats.set(tr);
            }
        });
    });

    // Window focus listener to refresh system theme immediately when switching back to app
    create_effect(move |_| {
        if let Some(win) = leptos::web_sys::window() {
            let cb = wasm_bindgen::closure::Closure::<dyn Fn()>::new(move || {
                leptos::spawn_local(async move {
                    let fresh = tauri_bridge::get_system_theme_info().await;
                    if sys_theme_info.get_untracked() != fresh {
                        set_sys_theme_info.set(fresh);
                    }
                });
            });
            let _ = win.add_event_listener_with_callback("focus", wasm_bindgen::JsCast::unchecked_ref(cb.as_ref()));
            cb.forget();
        }
    });

    create_effect(move |_| {
        if is_initialized.get() && status.get().state == SyncState::Locked {
            if let Some(el) = password_input_ref.get() {
                let _ = el.focus();
            }
        }
    });

    let on_toggle_pause = move |_| {
        let next = !is_paused.get();
        set_is_paused.set(next);
        set_status.update(|s| {
            s.state = if next {
                SyncState::Paused
            } else {
                SyncState::Idle
            };
        });
        leptos::spawn_local(async move {
            tauri_bridge::toggle_pause(next).await;
            let fresh = tauri_bridge::get_sync_state().await;
            set_status.set(fresh);
        });
    };

    let on_sync_now = move |_| {
        set_status.update(|s| {
            s.state = SyncState::Syncing;
        });
        set_is_syncing_anim.set(true);
        leptos::spawn_local(async move {
            tauri_bridge::trigger_sync().await;
            let fresh_st = tauri_bridge::get_sync_state().await;
            set_status.set(fresh_st);
            let fresh_stats = tauri_bridge::get_transfer_stats().await;
            set_stats.set(fresh_stats);
            gloo_timers::future::TimeoutFuture::new(1000).await;
            set_is_syncing_anim.set(false);
        });
    };

    let on_open_folder = move |_| {
        leptos::spawn_local(async move {
            tauri_bridge::open_cloud_folder().await;
        });
    };

    let on_toggle_folder = move |rel_path: String| {
        set_sync_folders.update(|folders| {
            for f in folders.iter_mut() {
                if f.rel_path == rel_path {
                    f.selected = !f.selected;
                }
            }
        });

        let selected: Vec<String> = sync_folders
            .get_untracked()
            .into_iter()
            .filter(|f| f.selected)
            .map(|f| f.rel_path)
            .collect();

        leptos::spawn_local(async move {
            let _ = tauri_bridge::set_sync_folders(selected).await;
        });
    };

    let on_toggle_sync_hidden_files = move || {
        let next = !sync_hidden_files.get();
        set_sync_hidden_files.set(next);
        leptos::spawn_local(async move {
            let _ = tauri_bridge::set_sync_hidden_files(next).await;
        });
    };

    let on_save_api_credentials = move |_| {
        let cid = custom_client_id.get();
        let cs = custom_client_secret.get();
        set_api_saving.set(true);
        set_api_feedback_msg.set(None);
        leptos::spawn_local(async move {
            match tauri_bridge::set_drive_api_credentials(cid, cs).await {
                Ok(_) => {
                    set_api_saving.set(false);
                    set_api_feedback_msg.set(Some((false, lang.get().settings_api_saved_msg().to_string())));
                }
                Err(e) => {
                    set_api_saving.set(false);
                    set_api_feedback_msg.set(Some((true, e)));
                }
            }
        });
    };

    let on_auth_with_api_credentials = move |_| {
        let cid = custom_client_id.get();
        let cs = custom_client_secret.get();
        set_api_authorizing.set(true);
        set_api_feedback_msg.set(Some((false, lang.get().settings_api_authorizing_msg().to_string())));
        leptos::spawn_local(async move {
            if let Err(e) = tauri_bridge::set_drive_api_credentials(cid, cs).await {
                set_api_authorizing.set(false);
                set_api_feedback_msg.set(Some((true, e)));
                return;
            }
            match tauri_bridge::trigger_drive_auth().await {
                Ok(_) => {
                    set_api_authorizing.set(false);
                    set_api_feedback_msg.set(Some((false, lang.get().settings_api_auth_success_msg().to_string())));
                    let fresh_st = tauri_bridge::get_sync_state().await;
                    set_status.set(fresh_st);
                }
                Err(e) => {
                    set_api_authorizing.set(false);
                    set_api_feedback_msg.set(Some((true, e)));
                }
            }
        });
    };

    let on_setup_finished = move || {
        set_is_setup_done.set(true);
        leptos::spawn_local(async move {
            let fresh_st = tauri_bridge::get_sync_state().await;
            set_status.set(fresh_st);
        });
    };

    let (unlock_password, set_unlock_password) = create_signal(String::new());
    let (unlock_loading, set_unlock_loading) = create_signal(false);
    let (unlock_error, set_unlock_error) = create_signal(Option::<String>::None);

    let on_unlock_vault = move || {
        let pw = unlock_password.get();
        if pw.is_empty() {
            return;
        }
        set_unlock_loading.set(true);
        set_unlock_error.set(None);
        leptos::spawn_local(async move {
            match tauri_bridge::unlock_vault(pw).await {
                Ok(_) => {
                    set_unlock_loading.set(false);
                    set_unlock_password.set(String::new());
                    set_status.update(|s| s.state = SyncState::Idle);
                    let fresh_st = tauri_bridge::get_sync_state().await;
                    set_status.set(fresh_st);
                }
                Err(err) => {
                    set_unlock_loading.set(false);
                    set_unlock_error.set(Some(err));
                }
            }
        });
    };

    let (recovery_mode, set_recovery_mode) = create_signal(false);
    let (recovery_key_input, set_recovery_key_input) = create_signal(String::new());
    let (recovery_new_pw, set_recovery_new_pw) = create_signal(String::new());
    let (recovery_loading, set_recovery_loading) = create_signal(false);
    let (recovery_error, set_recovery_error) = create_signal(Option::<String>::None);

    let on_recover_vault = move || {
        let rk = recovery_key_input.get();
        let np = recovery_new_pw.get();
        if rk.is_empty() || np.is_empty() {
            return;
        }
        if np.len() < 8 {
            set_recovery_error.set(Some(lang.get().wizard_pw_short().to_string()));
            return;
        }
        set_recovery_loading.set(true);
        set_recovery_error.set(None);
        leptos::spawn_local(async move {
            match tauri_bridge::recover_vault(rk, np).await {
                Ok(_) => {
                    set_recovery_loading.set(false);
                    set_recovery_key_input.set(String::new());
                    set_recovery_new_pw.set(String::new());
                    set_recovery_mode.set(false);
                    set_status.update(|s| s.state = SyncState::Idle);
                    let fresh_st = tauri_bridge::get_sync_state().await;
                    set_status.set(fresh_st);
                }
                Err(err) => {
                    set_recovery_loading.set(false);
                    set_recovery_error.set(Some(err));
                }
            }
        });
    };

    let on_lock_vault = move |_| {
        set_status.update(|s| s.state = SyncState::Locked);
        leptos::spawn_local(async move {
            let _ = tauri_bridge::lock_vault().await;
            let fresh = tauri_bridge::get_sync_state().await;
            set_status.set(fresh);
        });
    };

    let (change_current_pw, set_change_current_pw) = create_signal(String::new());
    let (change_new_pw, set_change_new_pw) = create_signal(String::new());
    let (change_confirm_pw, set_change_confirm_pw) = create_signal(String::new());
    let (change_pw_saving, set_change_pw_saving) = create_signal(false);
    let (change_pw_feedback_msg, set_change_pw_feedback_msg) = create_signal(Option::<(bool, String)>::None);

    let on_change_passphrase = move || {
        let cur_pw = change_current_pw.get();
        let new_pw = change_new_pw.get();
        let confirm_pw = change_confirm_pw.get();
        if cur_pw.is_empty() || new_pw.is_empty() || confirm_pw.is_empty() {
            return;
        }
        if new_pw.len() < 8 {
            set_change_pw_feedback_msg.set(Some((true, lang.get().wizard_pw_short().to_string())));
            return;
        }
        if new_pw != confirm_pw {
            set_change_pw_feedback_msg.set(Some((true, lang.get().wizard_pw_mismatch().to_string())));
            return;
        }
        set_change_pw_saving.set(true);
        set_change_pw_feedback_msg.set(None);
        leptos::spawn_local(async move {
            match tauri_bridge::change_passphrase(cur_pw, new_pw).await {
                Ok(_) => {
                    set_change_pw_saving.set(false);
                    set_change_current_pw.set(String::new());
                    set_change_new_pw.set(String::new());
                    set_change_confirm_pw.set(String::new());
                    set_change_pw_feedback_msg.set(Some((false, lang.get().settings_pw_changed_msg().to_string())));
                }
                Err(e) => {
                    set_change_pw_saving.set(false);
                    let translated = lang.get().translate_error(&e);
                    set_change_pw_feedback_msg.set(Some((true, translated)));
                }
            }
        });
    };

    let select_theme_mode = move |mode: &'static str| {
        set_theme_mode.set(mode.to_string());
        set_storage_item("aksync_theme_mode", mode);
    };

    let toggle_custom_theme = move |enabled: bool| {
        set_custom_theme_enabled.set(enabled);
        set_storage_item("aksync_custom_theme_enabled", if enabled { "true" } else { "false" });
        if enabled && get_storage_item("aksync_custom_window_bg").is_none() {
            let info = sys_theme_info.get_untracked();
            set_custom_window_bg.set(info.window_bg.clone());
            set_storage_item("aksync_custom_window_bg", &info.window_bg);
            set_custom_card_bg.set(info.card_bg.clone());
            set_storage_item("aksync_custom_card_bg", &info.card_bg);
            set_custom_accent_color.set(info.accent_color.clone());
            set_storage_item("aksync_custom_accent_color", &info.accent_color);
            set_custom_text_color.set(info.text_primary.clone());
            set_storage_item("aksync_custom_text_color", &info.text_primary);
        }
    };

    let reset_to_system_colors = move |_| {
        let info = sys_theme_info.get_untracked();
        set_custom_window_bg.set(info.window_bg.clone());
        set_storage_item("aksync_custom_window_bg", &info.window_bg);
        set_custom_card_bg.set(info.card_bg.clone());
        set_storage_item("aksync_custom_card_bg", &info.card_bg);
        set_custom_accent_color.set(info.accent_color.clone());
        set_storage_item("aksync_custom_accent_color", &info.accent_color);
        set_custom_text_color.set(info.text_primary.clone());
        set_storage_item("aksync_custom_text_color", &info.text_primary);
    };

    view! {
        <div
            class=move || {
                let de_class = if sys_theme_info.get().desktop_env.to_uppercase().contains("KDE") {
                    "de-kde"
                } else {
                    "de-gnome"
                };

                let theme_class = if custom_theme_enabled.get() {
                    if is_color_dark(&custom_window_bg.get()) { "theme-dark" } else { "theme-light" }
                } else {
                    match theme_mode.get().as_str() {
                        "dark" => "theme-dark",
                        "light" => "theme-light",
                        _ => if sys_theme_info.get().is_dark { "theme-dark" } else { "theme-light" }
                    }
                };

                format!("app-root {} {}", theme_class, de_class)
            }
            style=move || {
                let info = sys_theme_info.get();
                let is_custom = custom_theme_enabled.get();

                let (w_bg, v_bg, c_bg, acc, text_p, text_s, border) = if is_custom {
                    let w = custom_window_bg.get();
                    let c = custom_card_bg.get();
                    let acc = custom_accent_color.get();
                    let tp = custom_text_color.get();
                    let dark = is_color_dark(&w);
                    let ts = if dark { "rgba(255, 255, 255, 0.7)".to_string() } else { "rgba(0, 0, 0, 0.65)".to_string() };
                    let brd = if dark { "rgba(255, 255, 255, 0.12)".to_string() } else { "rgba(0, 0, 0, 0.12)".to_string() };
                    (w.clone(), w.clone(), c, acc, tp, ts, brd)
                } else {
                    let effective_dark = match theme_mode.get().as_str() {
                        "dark" => true,
                        "light" => false,
                        _ => info.is_dark,
                    };

                    let (w_bg, v_bg, c_bg, text_p, text_s, border) = if theme_mode.get() == "system" || effective_dark == info.is_dark {
                        (info.window_bg, info.view_bg, info.card_bg, info.text_primary, info.text_secondary, info.border_color)
                    } else if effective_dark {
                        ("#232629".to_string(), "#1b1e20".to_string(), "#31363b".to_string(), "#eff0f1".to_string(), "#bdc3c7".to_string(), "rgba(255, 255, 255, 0.12)".to_string())
                    } else {
                        ("#eff0f1".to_string(), "#fcfcfc".to_string(), "#ffffff".to_string(), "#232629".to_string(), "#6e747b".to_string(), "rgba(0, 0, 0, 0.12)".to_string())
                    };
                    (w_bg, v_bg, c_bg, info.accent_color, text_p, text_s, border)
                };

                let factor = match ui_scale.get().as_str() {
                    "90" => "0.9",
                    "100" => "1.0",
                    "110" => "1.1",
                    "125" => "1.25",
                    "150" => "1.5",
                    _ => "1.0",
                };

                format!(
                    "--ui-scale: {}; --window-bg: {}; --view-bg: {}; --card-bg: {}; --accent-color: {}; --text-primary: {}; --text-secondary: {}; --card-border: {}; background: {}; color: {};",
                    factor, w_bg, v_bg, c_bg, acc, text_p, text_s, border, w_bg, text_p
                )
            }
        >
            // Durum A: SetupRequired / İlk Kurulum (Tam pencere ortalanmış kurulum görünümü)
            <div
                class="centered-view-wrapper"
                style=move || if is_initialized.get() && (!is_setup_done.get() || matches!(status.get().state, SyncState::SetupRequired)) {
                    "display: flex;"
                } else {
                    "display: none;"
                }
            >
                <SetupWizard
                    lang=lang
                    step1_done=step1_done
                    set_step1_done=set_step1_done
                    password=password
                    set_password=set_password
                    confirm_password=confirm_password
                    set_confirm_password=set_confirm_password
                    on_setup_finished=on_setup_finished
                />
            </div>

            // Durum B: SyncState::Locked / Kasa Kilitli (Tam pencere ortalanmış kasa kilidi açma)
            <div
                class="centered-view-wrapper"
                style=move || if is_initialized.get() && is_setup_done.get() && matches!(status.get().state, SyncState::Locked) {
                    "display: flex;"
                } else {
                    "display: none;"
                }
            >
                <div class="unlock-card">
                    <div class="unlock-icon">
                        <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                            <rect width="18" height="11" x="3" y="11" rx="2" ry="2"/><path d="M7 11V7a5 5 0 0 1 10 0v4"/>
                        </svg>
                    </div>
                    <h2 style="font-size: 1.15rem; font-weight: 700; color: var(--text-primary); margin-bottom: 0.35rem;">
                        {move || lang.get().vault_locked_title()}
                    </h2>
                    <p style="color: var(--text-secondary); font-size: 0.8rem; margin-bottom: 1.25rem; line-height: 1.45;">
                        {move || lang.get().vault_locked_subtitle()}
                    </p>

                    {move || if recovery_mode.get() {
                        view! {
                            <>
                                <div style="display: flex; flex-direction: column; gap: 0.75rem; margin-bottom: 1rem;">
                                    <input
                                        type="text"
                                        class="wizard-input"
                                        style="font-family: monospace; font-size: 0.82rem;"
                                        placeholder=move || lang.get().recovery_key_placeholder()
                                        prop:value=recovery_key_input
                                        on:input=move |ev| set_recovery_key_input.set(event_target_value(&ev))
                                    />
                                    <input
                                        type="password"
                                        class="wizard-input"
                                        placeholder=move || lang.get().new_password_placeholder()
                                        prop:value=recovery_new_pw
                                        on:input=move |ev| set_recovery_new_pw.set(event_target_value(&ev))
                                        on:keydown=move |ev: web_sys::KeyboardEvent| {
                                            if ev.key() == "Enter" {
                                                on_recover_vault();
                                            }
                                        }
                                    />
                                </div>

                                {move || recovery_error.get().map(|err| view! {
                                    <div class="wizard-status-msg wizard-msg-error" style="margin-bottom: 1rem;">
                                        {lang.get().translate_error(&err)}
                                    </div>
                                })}

                                <button
                                    class="btn btn-primary btn-full"
                                    style="padding: 0.65rem;"
                                    disabled=move || recovery_loading.get() || recovery_key_input.get().trim().is_empty() || recovery_new_pw.get().is_empty()
                                    on:click=move |_| on_recover_vault()
                                >
                                    {move || if recovery_loading.get() {
                                        view! {
                                            <>
                                                <span class="spinner"></span>
                                                <span>{lang.get().recover_vault_btn()}</span>
                                            </>
                                        }.into_view()
                                    } else {
                                        view! {
                                            <>
                                                <span>{lang.get().recover_vault_btn()}</span>
                                            </>
                                        }.into_view()
                                    }}
                                </button>

                                <div style="margin-top: 1rem; text-align: center;">
                                    <button
                                        class="btn"
                                        style="font-size: 0.78rem; text-decoration: underline; color: var(--text-secondary); background: none; border: none; cursor: pointer; padding: 0.3rem 0.5rem;"
                                        on:click=move |_| {
                                            set_unlock_error.set(None);
                                            set_recovery_mode.set(false);
                                        }
                                    >
                                        {move || lang.get().back_to_unlock_link()}
                                    </button>
                                </div>
                            </>
                        }.into_view()
                    } else {
                        view! {
                            <>
                                <div style="margin-bottom: 1rem;">
                                    <input
                                        type="password"
                                        class="wizard-input"
                                        placeholder=move || lang.get().unlock_placeholder()
                                        prop:value=unlock_password
                                        autofocus=true
                                        node_ref=password_input_ref
                                        on:input=move |ev| set_unlock_password.set(event_target_value(&ev))
                                        on:keydown=move |ev: web_sys::KeyboardEvent| {
                                            if ev.key() == "Enter" {
                                                on_unlock_vault();
                                            }
                                        }
                                    />
                                </div>

                                {move || unlock_error.get().map(|err| view! {
                                    <div class="wizard-status-msg wizard-msg-error" style="margin-bottom: 1rem;">
                                        {lang.get().translate_error(&err)}
                                    </div>
                                })}

                                <button
                                    class="btn btn-primary btn-full"
                                    style="padding: 0.65rem;"
                                    disabled=move || unlock_loading.get() || unlock_password.get().is_empty()
                                    on:click=move |_| on_unlock_vault()
                                >
                                    {move || if unlock_loading.get() {
                                        view! {
                                            <>
                                                <span class="spinner"></span>
                                                <span>{lang.get().unlocking_btn()}</span>
                                            </>
                                        }.into_view()
                                    } else {
                                        view! {
                                            <>
                                                <span>{lang.get().unlock_btn()}</span>
                                            </>
                                        }.into_view()
                                    }}
                                </button>

                                <div style="margin-top: 1rem; text-align: center;">
                                    <button
                                        class="btn"
                                        style="font-size: 0.78rem; text-decoration: underline; color: var(--text-secondary); background: none; border: none; cursor: pointer; padding: 0.3rem 0.5rem;"
                                        on:click=move |_| {
                                            set_unlock_error.set(None);
                                            set_recovery_mode.set(true);
                                        }
                                    >
                                        {move || lang.get().forgot_password_link()}
                                    </button>
                                </div>
                            </>
                        }.into_view()
                    }}
                </div>
            </div>

            // Durum C: Kasa Açık (Edge-to-Edge Split-View Düzeni)
            <div
                class="desktop-split-layout"
                style=move || if is_initialized.get() && is_setup_done.get() && !matches!(status.get().state, SyncState::Locked | SyncState::SetupRequired) {
                    "display: flex;"
                } else {
                    "display: none;"
                }
            >
                // Sol Kenar Çubuğu (.sidebar)
                <aside class="sidebar">
                    <div class="sidebar-nav">
                        <button
                            class=move || if active_tab.get() == ActiveTab::SyncFolders { "nav-item active" } else { "nav-item" }
                            on:click=move |_| set_active_tab.set(ActiveTab::SyncFolders)
                        >
                            <span class="nav-icon">
                                <svg width="17" height="17" style="width: 1.15rem; height: 1.15rem;" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                    <path d="M21 12a9 9 0 0 0-9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"/><path d="M3 3v5h5"/><path d="M3 12a9 9 0 0 0 9 9 9.75 9.75 0 0 0 6.74-2.74L21 16"/><path d="M16 21h5v-5"/>
                                </svg>
                            </span>
                            <span>{move || lang.get().nav_sync_folders()}</span>
                        </button>
                        <button
                            class=move || if active_tab.get() == ActiveTab::VaultSecurity { "nav-item active" } else { "nav-item" }
                            on:click=move |_| set_active_tab.set(ActiveTab::VaultSecurity)
                        >
                            <span class="nav-icon">
                                <svg width="17" height="17" style="width: 1.15rem; height: 1.15rem;" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                    <path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"/>
                                </svg>
                            </span>
                            <span>{move || lang.get().nav_vault_security()}</span>
                        </button>
                        <button
                            class=move || if active_tab.get() == ActiveTab::AppSettings { "nav-item active" } else { "nav-item" }
                            on:click=move |_| set_active_tab.set(ActiveTab::AppSettings)
                        >
                            <span class="nav-icon">
                                <svg width="17" height="17" style="width: 1.15rem; height: 1.15rem;" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                    <path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"/>
                                    <circle cx="12" cy="12" r="3"/>
                                </svg>
                            </span>
                            <span>{move || lang.get().nav_app_settings()}</span>
                        </button>
                    </div>

                    // Sol Alt Depolama ve Kilit Kutusu (.sidebar-footer)
                    <div class="sidebar-footer">
                        {move || {
                            let is_loading = matches!(status.get().state, SyncState::Connecting) || stats.get().storage_total == 0;
                            if is_loading {
                                view! {
                                    <div class="storage-status-row">
                                        <span>{lang.get().storage_short_label()}: "..."</span>
                                        <span class="storage-pct-badge">"..."</span>
                                    </div>
                                }.into_view()
                            } else {
                                let s = stats.get();
                                let used_str = format_bytes(s.storage_used);
                                let total_str = format_bytes(s.storage_total);
                                let pct = (s.storage_used as f64 / s.storage_total as f64) * 100.0;
                                view! {
                                    <div class="storage-status-row">
                                        <span>{lang.get().storage_short_label()}: {format!("{} / {}", used_str, total_str)}</span>
                                        <span class="storage-pct-badge">{format!("{:.0}%", pct.min(100.0))}</span>
                                    </div>
                                }.into_view()
                            }
                        }}
                        <div class="progress-bar-container">
                            {move || {
                                let is_loading = matches!(status.get().state, SyncState::Connecting) || stats.get().storage_total == 0;
                                if is_loading {
                                    view! { <div class="progress-bar skeleton" style="width: 100%;"></div> }.into_view()
                                } else {
                                    let s = stats.get();
                                    let pct = (s.storage_used as f64 / s.storage_total as f64) * 100.0;
                                    view! {
                                        <div
                                            class="progress-bar"
                                            style=format!("width: {:.1}%;", pct.min(100.0))
                                        ></div>
                                    }.into_view()
                                }
                            }}
                        </div>
                        <button
                            class="btn btn-full"
                            title=move || lang.get().lock_vault_btn()
                            on:click=on_lock_vault
                        >
                            <svg width="15" height="15" style="width: 1rem; height: 1rem;" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                <rect width="18" height="11" x="3" y="11" rx="2" ry="2"/><path d="M7 11V7a5 5 0 0 1 10 0v4"/>
                            </svg>
                            <span>{move || lang.get().lock_vault_btn()}</span>
                        </button>
                    </div>
                </aside>

                // Sağ İçerik Bölmesi (.content-pane)
                <main class="content-pane">
                    // Sekme 1: Eşitleme ve Klasörler
                    <div style=move || if active_tab.get() == ActiveTab::SyncFolders { "display: flex; flex-direction: column; gap: 1.1rem;" } else { "display: none;" }>
                        // Üst Durum ve Aksiyon Şeridi (.status-toolbar)
                        <div class="status-toolbar">
                            <div class="status-left">
                                {move || {
                                    let st = status.get();
                                    let dot_class = match st.state {
                                        SyncState::SetupRequired => "status-dot dot-paused",
                                        SyncState::Locked => "status-dot dot-paused",
                                        SyncState::Connecting => "status-dot dot-connecting",
                                        SyncState::Idle => "status-dot dot-idle",
                                        SyncState::Syncing => "status-dot dot-syncing",
                                        SyncState::Paused => "status-dot dot-paused",
                                        SyncState::Offline => "status-dot dot-offline",
                                        SyncState::Error(_) => "status-dot dot-error",
                                    };
                                    view! { <span class=dot_class></span> }
                                }}
                                <span class="status-label">
                                    {move || {
                                        let st = status.get();
                                        match st.state {
                                            SyncState::SetupRequired => lang.get().state_setup_required().to_string(),
                                            SyncState::Locked => lang.get().state_locked().to_string(),
                                            SyncState::Connecting => lang.get().state_connecting().to_string(),
                                            SyncState::Idle => lang.get().state_idle().to_string(),
                                            SyncState::Syncing => lang.get().state_syncing().to_string(),
                                            SyncState::Paused => lang.get().state_paused().to_string(),
                                            SyncState::Offline => lang.get().state_offline().to_string(),
                                            SyncState::Error(err) => err,
                                        }
                                    }}
                                </span>
                                {move || {
                                    let s = stats.get();
                                    if s.download_speed > 0 || s.upload_speed > 0 {
                                        view! {
                                            <span class="speed-badge">
                                                {format!("↓ {} ↑ {}", format_speed(s.download_speed), format_speed(s.upload_speed))}
                                            </span>
                                        }.into_view()
                                    } else {
                                        ().into_view()
                                    }
                                }}
                            </div>

                            <div class="status-actions">
                                <button class="btn" on:click=on_open_folder>
                                    <svg width="15" height="15" style="width: 1rem; height: 1rem;" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                        <path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/>
                                    </svg>
                                    <span>{move || lang.get().open_folder_btn()}</span>
                                </button>
                                <button class="btn" on:click=on_sync_now>
                                    <svg class=move || if is_syncing_anim.get() || matches!(status.get().state, SyncState::Syncing) { "spin-anim" } else { "" } width="15" height="15" style="width: 1rem; height: 1rem;" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                        <path d="M21 12a9 9 0 0 0-9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"/><path d="M3 3v5h5"/><path d="M3 12a9 9 0 0 0 9 9 9.75 9.75 0 0 0 6.74-2.74L21 16"/><path d="M16 21h5v-5"/>
                                    </svg>
                                    <span>{move || lang.get().sync_now_btn()}</span>
                                </button>
                                <button
                                    class="btn"
                                    on:click=on_toggle_pause
                                >
                                    {move || if is_paused.get() {
                                        view! {
                                            <>
                                                <svg width="15" height="15" style="width: 1rem; height: 1rem;" viewBox="0 0 24 24" fill="currentColor"><polygon points="6 3 20 12 6 21 6 3"/></svg>
                                                <span>{lang.get().resume_short_btn()}</span>
                                            </>
                                        }
                                    } else {
                                        view! {
                                            <>
                                                <svg width="15" height="15" style="width: 1rem; height: 1rem;" viewBox="0 0 24 24" fill="currentColor"><rect x="6" y="4" width="4" height="16"/><rect x="14" y="4" width="4" height="16"/></svg>
                                                <span>{lang.get().pause_short_btn()}</span>
                                            </>
                                        }
                                    }}
                                </button>
                            </div>
                        </div>

                        // Eşitlenen Ev Klasörleri Bölümü
                        <div class="content-section">
                            <div class="section-header-block">
                                <h3 class="section-title">{move || lang.get().synced_home_folders_title()}</h3>
                                <p class="section-subtitle">{move || lang.get().synced_home_folders_subtitle()}</p>
                            </div>

                            <div class="inset-list-box">
                                {move || {
                                    let list = sync_folders.get();
                                    if list.is_empty() {
                                        view! {
                                            <div style="padding: 1rem; text-align: center; color: var(--text-muted); font-size: 0.8rem;">
                                                {lang.get().sync_folders_empty()}
                                            </div>
                                        }.into_view()
                                    } else {
                                        view! {
                                            <For
                                                each=move || sync_folders.get()
                                                key=|folder| folder.rel_path.clone()
                                                children=move |folder| {
                                                    let rel_path = folder.rel_path.clone();
                                                    let rel_path_toggle = rel_path.clone();
                                                    let is_selected = folder.selected;
                                                    let icon_name = folder.icon.clone();
                                                    let folder_name = folder.name.clone();
                                                    let abs_path = folder.abs_path.clone();

                                                    view! {
                                                        <div
                                                            class="folder-row"
                                                            on:click=move |_| on_toggle_folder(rel_path_toggle.clone())
                                                        >
                                                            <div class="folder-meta-left">
                                                                <span class="folder-icon-wrap">
                                                                    {render_folder_icon(&icon_name)}
                                                                </span>
                                                                <div class="folder-name-col">
                                                                    <span class="folder-name">{folder_name}</span>
                                                                    <span class="folder-path">{abs_path}</span>
                                                                </div>
                                                            </div>
                                                            <input
                                                                type="checkbox"
                                                                class="native-checkbox"
                                                                prop:checked=is_selected
                                                                on:click=move |ev: web_sys::MouseEvent| ev.stop_propagation()
                                                                on:change=move |_| on_toggle_folder(rel_path.clone())
                                                            />
                                                        </div>
                                                    }
                                                }
                                            />
                                        }.into_view()
                                    }
                                }}
                            </div>

                            <label class="checkbox-line">
                                <input
                                    type="checkbox"
                                    class="native-checkbox"
                                    prop:checked=move || sync_hidden_files.get()
                                    on:change=move |_| on_toggle_sync_hidden_files()
                                />
                                <span>{move || lang.get().sync_hidden_checkbox_label()}</span>
                            </label>
                        </div>
                    </div>

                    // Sekme 2: Kasa Güvenliği
                    <div style=move || if active_tab.get() == ActiveTab::VaultSecurity { "display: flex; flex-direction: column; gap: 1.1rem;" } else { "display: none;" }>
                        <div class="content-section">
                            <div class="section-header-block">
                                <h3 class="section-title">{move || lang.get().settings_change_pw_title()}</h3>
                                <p class="section-subtitle">{move || lang.get().settings_change_pw_desc()}</p>
                            </div>

                            <div class="pref-box">
                                <div class="pref-form">
                                    <div class="pref-input-row">
                                        <label class="pref-input-label">{move || lang.get().settings_current_pw_placeholder()}</label>
                                        <input
                                            type="password"
                                            class="pref-input"
                                            placeholder=move || lang.get().settings_current_pw_placeholder()
                                            prop:value=change_current_pw
                                            on:input=move |ev| set_change_current_pw.set(event_target_value(&ev))
                                        />
                                    </div>

                                    <div class="pref-input-row">
                                        <label class="pref-input-label">{move || lang.get().settings_new_pw_placeholder()}</label>
                                        <input
                                            type="password"
                                            class="pref-input"
                                            placeholder=move || lang.get().settings_new_pw_placeholder()
                                            prop:value=change_new_pw
                                            on:input=move |ev| set_change_new_pw.set(event_target_value(&ev))
                                        />
                                    </div>

                                    <div class="pref-input-row">
                                        <label class="pref-input-label">{move || lang.get().confirm_new_password_placeholder()}</label>
                                        <input
                                            type="password"
                                            class="pref-input"
                                            placeholder=move || lang.get().confirm_new_password_placeholder()
                                            prop:value=change_confirm_pw
                                            on:input=move |ev| set_change_confirm_pw.set(event_target_value(&ev))
                                            on:keydown=move |ev: web_sys::KeyboardEvent| {
                                                if ev.key() == "Enter" {
                                                    on_change_passphrase();
                                                }
                                            }
                                        />
                                    </div>

                                    <div style="display: flex; align-items: center; justify-content: flex-end; margin-top: 0.35rem;">
                                        <button
                                            class="btn btn-primary"
                                            disabled=move || change_pw_saving.get() || change_current_pw.get().is_empty() || change_new_pw.get().is_empty() || change_confirm_pw.get().is_empty()
                                            on:click=move |_| on_change_passphrase()
                                        >
                                            {move || if change_pw_saving.get() {
                                                view! { <span class="spinner"></span> <span>{lang.get().settings_change_pw_btn()}</span> }.into_view()
                                            } else {
                                                view! {
                                                    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                                        <rect width="18" height="11" x="3" y="11" rx="2" ry="2"/><path d="M7 11V7a5 5 0 0 1 10 0v4"/>
                                                    </svg>
                                                    <span>{lang.get().settings_change_pw_btn()}</span>
                                                }.into_view()
                                            }}
                                        </button>
                                    </div>

                                    {move || change_pw_feedback_msg.get().map(|(is_err, msg)| {
                                        let msg_class = if is_err { "wizard-status-msg wizard-msg-error" } else { "wizard-status-msg wizard-msg-success" };
                                        view! {
                                            <div class=msg_class style="margin-top: 0.4rem;">
                                                {msg}
                                            </div>
                                        }
                                    })}
                                </div>
                            </div>
                        </div>
                    </div>

                    // Sekme 3: Uygulama Ayarları
                    <div style=move || if active_tab.get() == ActiveTab::AppSettings { "display: flex; flex-direction: column; gap: 1.1rem;" } else { "display: none;" }>
                        // Görünüm ve Dil
                        <div class="content-section">
                            <div class="section-header-block">
                                <h3 class="section-title">{move || lang.get().settings_appearance_section()}</h3>
                                <p class="section-subtitle">"Arayüz dili, masaüstü ortamı teması ve renk tercihleri"</p>
                            </div>

                            <div class="pref-box">
                                // Dil Seçimi
                                <div class="pref-row">
                                    <div class="pref-row-meta">
                                        <span class="pref-row-label">{move || lang.get().settings_language()}</span>
                                        <span class="pref-row-desc">"Uygulama arayüz dili (Türkçe / English)"</span>
                                    </div>
                                    <div class="segmented-control">
                                        <button
                                            type="button"
                                            class=move || if lang.get() == Language::Turkish { "segmented-btn active" } else { "segmented-btn" }
                                            on:click=move |_| set_lang.set(Language::Turkish)
                                        >
                                            "TR"
                                        </button>
                                        <button
                                            type="button"
                                            class=move || if lang.get() == Language::English { "segmented-btn active" } else { "segmented-btn" }
                                            on:click=move |_| set_lang.set(Language::English)
                                        >
                                            "EN"
                                        </button>
                                    </div>
                                </div>

                                // Tema Modu
                                <div class="pref-row">
                                    <div class="pref-row-meta">
                                        <span class="pref-row-label">{move || lang.get().settings_theme_mode()}</span>
                                        <span class="pref-row-desc">"Sistem temasıyla senkronize veya zorunlu açık/koyu mod"</span>
                                    </div>
                                    <div class="segmented-control">
                                        <button
                                            type="button"
                                            class=move || if theme_mode.get() == "system" { "segmented-btn active" } else { "segmented-btn" }
                                            on:click=move |_| select_theme_mode("system")
                                        >
                                            {move || lang.get().theme_system()}
                                        </button>
                                        <button
                                            type="button"
                                            class=move || if theme_mode.get() == "light" { "segmented-btn active" } else { "segmented-btn" }
                                            on:click=move |_| select_theme_mode("light")
                                        >
                                            {move || lang.get().theme_light()}
                                        </button>
                                        <button
                                            type="button"
                                            class=move || if theme_mode.get() == "dark" { "segmented-btn active" } else { "segmented-btn" }
                                            on:click=move |_| select_theme_mode("dark")
                                        >
                                            {move || lang.get().theme_dark()}
                                        </button>
                                    </div>
                                </div>

                                // Arayüz Ölçeği (UI Scale)
                                <div class="pref-row">
                                    <div class="pref-row-meta">
                                        <span class="pref-row-label">{move || lang.get().settings_ui_scale()}</span>
                                        <span class="pref-row-desc">{move || lang.get().settings_ui_scale_desc()}</span>
                                    </div>
                                    <div class="segmented-control">
                                        <button
                                            type="button"
                                            class=move || if ui_scale.get() == "90" { "segmented-btn active" } else { "segmented-btn" }
                                            on:click=move |_| select_ui_scale("90")
                                        >
                                            "90%"
                                        </button>
                                        <button
                                            type="button"
                                            class=move || if ui_scale.get() == "100" { "segmented-btn active" } else { "segmented-btn" }
                                            on:click=move |_| select_ui_scale("100")
                                        >
                                            "100%"
                                        </button>
                                        <button
                                            type="button"
                                            class=move || if ui_scale.get() == "110" { "segmented-btn active" } else { "segmented-btn" }
                                            on:click=move |_| select_ui_scale("110")
                                        >
                                            "110%"
                                        </button>
                                        <button
                                            type="button"
                                            class=move || if ui_scale.get() == "125" { "segmented-btn active" } else { "segmented-btn" }
                                            on:click=move |_| select_ui_scale("125")
                                        >
                                            "125%"
                                        </button>
                                        <button
                                            type="button"
                                            class=move || if ui_scale.get() == "150" { "segmented-btn active" } else { "segmented-btn" }
                                            on:click=move |_| select_ui_scale("150")
                                        >
                                            "150%"
                                        </button>
                                    </div>
                                </div>

                                // Özel Renkler Onay Kutusu
                                <div class="pref-row">
                                    <div class="pref-row-meta">
                                        <span class="pref-row-label">{move || lang.get().settings_custom_enable()}</span>
                                        <span class="pref-row-desc">{move || lang.get().settings_custom_theme_desc()}</span>
                                    </div>
                                    <input
                                        type="checkbox"
                                        class="native-checkbox"
                                        prop:checked=move || custom_theme_enabled.get()
                                        on:change=move |ev| {
                                            let checked = event_target_checked(&ev);
                                            toggle_custom_theme(checked);
                                        }
                                    />
                                </div>

                                {move || if custom_theme_enabled.get() {
                                    view! {
                                        <>
                                            <div class="pref-row">
                                                <div class="pref-row-meta">
                                                    <span class="pref-row-label">{lang.get().settings_custom_window_bg()}</span>
                                                </div>
                                                <div class="color-picker-control">
                                                    <input
                                                        type="color"
                                                        class="color-swatch-input"
                                                        prop:value=move || to_valid_hex_7(&custom_window_bg.get(), "#232629")
                                                        on:input=move |ev| {
                                                            let val = event_target_value(&ev);
                                                            set_custom_window_bg.set(val.clone());
                                                            set_storage_item("aksync_custom_window_bg", &val);
                                                        }
                                                    />
                                                    <input
                                                        type="text"
                                                        class="color-hex-input"
                                                        prop:value=move || custom_window_bg.get()
                                                        on:input=move |ev| {
                                                            let val = event_target_value(&ev);
                                                            set_custom_window_bg.set(val.clone());
                                                            set_storage_item("aksync_custom_window_bg", &val);
                                                        }
                                                    />
                                                </div>
                                            </div>

                                            <div class="pref-row">
                                                <div class="pref-row-meta">
                                                    <span class="pref-row-label">{lang.get().settings_custom_card_bg()}</span>
                                                </div>
                                                <div class="color-picker-control">
                                                    <input
                                                        type="color"
                                                        class="color-swatch-input"
                                                        prop:value=move || to_valid_hex_7(&custom_card_bg.get(), "#31363b")
                                                        on:input=move |ev| {
                                                            let val = event_target_value(&ev);
                                                            set_custom_card_bg.set(val.clone());
                                                            set_storage_item("aksync_custom_card_bg", &val);
                                                        }
                                                    />
                                                    <input
                                                        type="text"
                                                        class="color-hex-input"
                                                        prop:value=move || custom_card_bg.get()
                                                        on:input=move |ev| {
                                                            let val = event_target_value(&ev);
                                                            set_custom_card_bg.set(val.clone());
                                                            set_storage_item("aksync_custom_card_bg", &val);
                                                        }
                                                    />
                                                </div>
                                            </div>

                                            <div class="pref-row">
                                                <div class="pref-row-meta">
                                                    <span class="pref-row-label">{lang.get().settings_custom_accent()}</span>
                                                </div>
                                                <div class="color-picker-control">
                                                    <input
                                                        type="color"
                                                        class="color-swatch-input"
                                                        prop:value=move || to_valid_hex_7(&custom_accent_color.get(), "#3daee9")
                                                        on:input=move |ev| {
                                                            let val = event_target_value(&ev);
                                                            set_custom_accent_color.set(val.clone());
                                                            set_storage_item("aksync_custom_accent_color", &val);
                                                        }
                                                    />
                                                    <input
                                                        type="text"
                                                        class="color-hex-input"
                                                        prop:value=move || custom_accent_color.get()
                                                        on:input=move |ev| {
                                                            let val = event_target_value(&ev);
                                                            set_custom_accent_color.set(val.clone());
                                                            set_storage_item("aksync_custom_accent_color", &val);
                                                        }
                                                    />
                                                </div>
                                            </div>

                                            <div class="pref-row">
                                                <div class="pref-row-meta">
                                                    <span class="pref-row-label">{lang.get().settings_custom_text()}</span>
                                                </div>
                                                <div class="color-picker-control">
                                                    <input
                                                        type="color"
                                                        class="color-swatch-input"
                                                        prop:value=move || to_valid_hex_7(&custom_text_color.get(), "#eff0f1")
                                                        on:input=move |ev| {
                                                            let val = event_target_value(&ev);
                                                            set_custom_text_color.set(val.clone());
                                                            set_storage_item("aksync_custom_text_color", &val);
                                                        }
                                                    />
                                                    <input
                                                        type="text"
                                                        class="color-hex-input"
                                                        prop:value=move || custom_text_color.get()
                                                        on:input=move |ev| {
                                                            let val = event_target_value(&ev);
                                                            set_custom_text_color.set(val.clone());
                                                            set_storage_item("aksync_custom_text_color", &val);
                                                        }
                                                    />
                                                </div>
                                            </div>

                                            <div class="pref-row" style="justify-content: flex-end;">
                                                <button
                                                    class="btn"
                                                    on:click=reset_to_system_colors
                                                >
                                                    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                                        <path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"/><path d="M3 3v5h5"/>
                                                    </svg>
                                                    <span>{lang.get().settings_custom_reset()}</span>
                                                </button>
                                            </div>
                                        </>
                                    }.into_view()
                                } else {
                                    ().into_view()
                                }}
                            </div>
                        </div>

                        // Özel Google Drive API
                        <div class="content-section">
                            <div class="section-header-block">
                                <h3 class="section-title">{move || lang.get().settings_custom_api_title()}</h3>
                                <p class="section-subtitle">{move || lang.get().settings_custom_api_desc()}</p>
                            </div>

                            <div class="pref-box">
                                <div class="pref-form">
                                    <div class="pref-input-row">
                                        <label class="pref-input-label">{move || lang.get().settings_client_id_label()}</label>
                                        <input
                                            type="text"
                                            class="pref-input"
                                            placeholder="xxxx.apps.googleusercontent.com"
                                            prop:value=custom_client_id
                                            on:input=move |ev| set_custom_client_id.set(event_target_value(&ev))
                                        />
                                    </div>

                                    <div class="pref-input-row">
                                        <label class="pref-input-label">{move || lang.get().settings_client_secret_label()}</label>
                                        <input
                                            type="password"
                                            class="pref-input"
                                            placeholder="GOCSPX-xxxxxxxxxxxxxxxx"
                                            prop:value=custom_client_secret
                                            on:input=move |ev| set_custom_client_secret.set(event_target_value(&ev))
                                        />
                                    </div>

                                    <div style="display: flex; align-items: center; justify-content: flex-end; gap: 0.6rem; margin-top: 0.35rem;">
                                        <button
                                            class="btn"
                                            disabled=move || api_saving.get()
                                            on:click=on_save_api_credentials
                                        >
                                            {move || if api_saving.get() {
                                                view! { <span class="spinner"></span> <span>{lang.get().settings_api_save_btn()}</span> }.into_view()
                                            } else {
                                                view! {
                                                    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                                        <path d="M19 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11l5 5v11a2 2 0 0 1-2 2z"/><polyline points="17 21 17 13 7 13 7 21"/><polyline points="7 3 7 8 15 8"/>
                                                    </svg>
                                                    <span>{lang.get().settings_api_save_btn()}</span>
                                                }.into_view()
                                            }}
                                        </button>
                                        <button
                                            class="btn btn-primary"
                                            disabled=move || api_authorizing.get()
                                            on:click=on_auth_with_api_credentials
                                        >
                                            {move || if api_authorizing.get() {
                                                view! { <span class="spinner"></span> <span>{lang.get().wizard_connecting()}</span> }.into_view()
                                            } else {
                                                view! {
                                                    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                                        <path d="M15 3h4a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2h-4"/><polyline points="10 17 15 12 10 7"/><line x1="15" x2="3" y1="12" y2="12"/>
                                                    </svg>
                                                    <span>{lang.get().settings_api_auth_btn()}</span>
                                                }.into_view()
                                            }}
                                        </button>
                                    </div>

                                    {move || api_feedback_msg.get().map(|(is_err, msg)| {
                                        let msg_class = if is_err { "wizard-status-msg wizard-msg-error" } else { "wizard-status-msg wizard-msg-success" };
                                        view! {
                                            <div class=msg_class style="margin-top: 0.4rem;">
                                                {msg}
                                            </div>
                                        }
                                    })}
                                </div>
                            </div>
                        </div>
                    </div>
                </main>
            </div>
        </div>
    }
}
