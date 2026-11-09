use crate::i18n::Language;
use crate::tauri_bridge;
use leptos::*;

fn copy_text_to_clipboard(text: &str) {
    let Ok(js_str) = serde_json::to_string(text) else {
        return;
    };
    let js_code = format!(
        r#"(function(val) {{
            try {{
                if (navigator.clipboard && navigator.clipboard.writeText) {{
                    navigator.clipboard.writeText(val);
                }}
            }} catch (e) {{}}
            try {{
                var el = document.createElement('textarea');
                el.value = val;
                el.setAttribute('readonly', '');
                el.style.position = 'fixed';
                el.style.left = '-9999px';
                document.body.appendChild(el);
                el.select();
                document.execCommand('copy');
                document.body.removeChild(el);
            }} catch (e) {{}}
        }})({js_str})"#
    );
    let _ = js_sys::eval(&js_code);
}

#[component]
pub fn SetupWizard<F>(
    lang: ReadSignal<Language>,
    step1_done: ReadSignal<bool>,
    set_step1_done: WriteSignal<bool>,
    password: ReadSignal<String>,
    set_password: WriteSignal<String>,
    confirm_password: ReadSignal<String>,
    set_confirm_password: WriteSignal<String>,
    on_setup_finished: F,
) -> impl IntoView
where
    F: Fn() + Copy + 'static,
{
    let _ = step1_done;

    // Active wizard step: 1 = Cloud Storage, 2 = Vault Setup, 3 = Recovery Key Backup
    let (current_step, set_current_step) = create_signal(1u8);

    // Step 1 states
    let (drive_connected, set_drive_connected) = create_signal(false);
    let (step1_loading, set_step1_loading) = create_signal(false);
    let (step1_error, set_step1_error) = create_signal(Option::<String>::None);
    let (show_step1_advanced, set_show_step1_advanced) = create_signal(false);
    let (custom_client_id, set_custom_client_id) = create_signal(String::new());
    let (custom_client_secret, set_custom_client_secret) = create_signal(String::new());
    let (api_saving, set_api_saving) = create_signal(false);
    let (api_feedback_msg, set_api_feedback_msg) = create_signal(Option::<(bool, String)>::None);

    // Check if rclone Google Drive is already configured and load existing credentials
    create_effect(move |_| {
        leptos::spawn_local(async move {
            if let Ok(setup_info) = tauri_bridge::check_setup_status().await {
                if setup_info.rclone_configured {
                    set_drive_connected.set(true);
                }
            }
            if let Ok(creds) = tauri_bridge::get_drive_api_credentials().await {
                if !creds.client_id.is_empty() {
                    set_custom_client_id.set(creds.client_id);
                }
                if !creds.client_secret.is_empty() {
                    set_custom_client_secret.set(creds.client_secret);
                }
            }
        });
    });

    // Step 2 states
    let (scanning_vaults, set_scanning_vaults) = create_signal(false);
    let (remote_vaults, set_remote_vaults) = create_signal(Vec::<String>::new());
    let (selected_vault, set_selected_vault) = create_signal(String::new());

    // Step 2: Existing vault recovery states
    let (recovery_key_input, set_recovery_key_input) = create_signal(String::new());
    let (recovery_verifying, set_recovery_verifying) = create_signal(false);
    let (recovery_verified, set_recovery_verified) = create_signal(false);
    let (existing_new_pw, set_existing_new_pw) = create_signal(String::new());
    let (existing_confirm_pw, set_existing_confirm_pw) = create_signal(String::new());
    let (linking_loading, set_linking_loading) = create_signal(false);
    let (existing_vault_error, set_existing_vault_error) = create_signal(Option::<String>::None);

    // Step 2: Create new vault states
    let (show_create_new, set_show_create_new) = create_signal(false);
    let (new_vault_name, set_new_vault_name) = create_signal(String::new());
    let (step2_create_loading, set_step2_create_loading) = create_signal(false);
    let (step2_create_error, set_step2_create_error) = create_signal(Option::<String>::None);

    // Step 3 states
    let (generated_recovery_key, set_generated_recovery_key) = create_signal(String::new());
    let (recovery_countdown, set_recovery_countdown) = create_signal(0u8);
    let (step3_unlocking, set_step3_unlocking) = create_signal(false);
    let (step3_error, set_step3_error) = create_signal(Option::<String>::None);
    let (key_copied, set_key_copied) = create_signal(false);

    // Step 1: Connect to Google Drive OAuth
    let on_connect_drive = move |_| {
        set_step1_loading.set(true);
        set_step1_error.set(None);
        leptos::spawn_local(async move {
            match tauri_bridge::trigger_drive_auth().await {
                Ok(_) => {
                    set_step1_loading.set(false);
                    set_drive_connected.set(true);
                }
                Err(err) => {
                    set_step1_loading.set(false);
                    set_step1_error.set(Some(err));
                }
            }
        });
    };

    // Transition from Step 1 to Step 2: Scan Google Drive Aksync/ folder
    let on_go_to_step2 = move |_| {
        set_scanning_vaults.set(true);
        set_step1_done.set(true);
        set_current_step.set(2);
        leptos::spawn_local(async move {
            match tauri_bridge::list_remote_vaults().await {
                Ok(vaults) => {
                    if let Some(first) = vaults.first() {
                        set_selected_vault.set(first.clone());
                    }
                    set_remote_vaults.set(vaults);
                }
                Err(_) => {
                    set_remote_vaults.set(Vec::new());
                }
            }
            set_scanning_vaults.set(false);
        });
    };

    // Step 2 (Existing Vault): Verify 52-char recovery key
    let on_verify_recovery_key = move |_| {
        let v_name = selected_vault.get();
        let key = recovery_key_input.get().trim().to_string();
        if key.is_empty() {
            set_existing_vault_error.set(Some("Please enter the recovery key.".to_string()));
            return;
        }
        set_recovery_verifying.set(true);
        set_existing_vault_error.set(None);
        leptos::spawn_local(async move {
            match tauri_bridge::verify_cloud_vault_recovery_key(v_name, key).await {
                Ok(_) => {
                    set_recovery_verifying.set(false);
                    set_recovery_verified.set(true);
                }
                Err(e) => {
                    set_recovery_verifying.set(false);
                    set_existing_vault_error.set(Some(e));
                }
            }
        });
    };

    // Step 2 (Existing Vault): Link and unlock vault with new local password
    let on_link_and_unlock_vault = move |_| {
        let v_name = selected_vault.get();
        let key = recovery_key_input.get().trim().to_string();
        let pw = existing_new_pw.get();
        let confirm = existing_confirm_pw.get();

        if pw.len() < 8 {
            set_existing_vault_error.set(Some(lang.get().wizard_pw_short().to_string()));
            return;
        }
        if pw != confirm {
            set_existing_vault_error.set(Some(lang.get().wizard_pw_mismatch().to_string()));
            return;
        }

        set_linking_loading.set(true);
        set_existing_vault_error.set(None);
        leptos::spawn_local(async move {
            match tauri_bridge::recover_cloud_vault(v_name, key, pw).await {
                Ok(_) => {
                    set_linking_loading.set(false);
                    on_setup_finished();
                }
                Err(e) => {
                    set_linking_loading.set(false);
                    set_existing_vault_error.set(Some(e));
                }
            }
        });
    };

    // Step 2 (New Vault): Initialize new vault with custom name & password
    let on_create_new_vault = move |_| {
        let v_name = new_vault_name.get().trim().to_string();
        let pw = password.get();
        let confirm = confirm_password.get();

        if v_name.is_empty() {
            set_step2_create_error.set(Some(lang.get().wizard_vault_name_empty().to_string()));
            return;
        }

        let vault_exists_remotely = remote_vaults
            .get()
            .iter()
            .any(|r| r.eq_ignore_ascii_case(&v_name));
        if vault_exists_remotely {
            let err_msg = lang.get().translate_error("A vault with this name already exists in Google Drive. Please choose a different name or recover the existing vault.");
            set_step2_create_error.set(Some(err_msg));
            return;
        }

        if pw.len() < 8 {
            set_step2_create_error.set(Some(lang.get().wizard_pw_short().to_string()));
            return;
        }
        if pw != confirm {
            set_step2_create_error.set(Some(lang.get().wizard_pw_mismatch().to_string()));
            return;
        }

        set_step2_create_loading.set(true);
        set_step2_create_error.set(None);
        leptos::spawn_local(async move {
            match tauri_bridge::initialize_vault_named(v_name, pw).await {
                Ok(msg) => {
                    set_step2_create_loading.set(false);
                    let key = if let Some(pos) = msg.find("Recovery Key:") {
                        msg[pos + "Recovery Key:".len()..].trim().to_string()
                    } else {
                        msg.clone()
                    };
                    set_generated_recovery_key.set(key);
                    set_recovery_countdown.set(5);
                    set_current_step.set(3);
                    leptos::spawn_local(async move {
                        for remaining in (1..=5u8).rev() {
                            set_recovery_countdown.set(remaining);
                            gloo_timers::future::TimeoutFuture::new(1000).await;
                        }
                        set_recovery_countdown.set(0);
                    });
                }
                Err(e) => {
                    set_step2_create_loading.set(false);
                    set_step2_create_error.set(Some(lang.get().translate_error(&e)));
                }
            }
        });
    };

    // Step 3: Finish backup and unlock vault
    let on_finish_step3 = move |_| {
        if recovery_countdown.get() == 0 && !step3_unlocking.get() {
            set_step3_unlocking.set(true);
            set_step3_error.set(None);
            let pw = password.get();
            leptos::spawn_local(async move {
                match tauri_bridge::unlock_vault(pw).await {
                    Ok(_) => {
                        set_step3_unlocking.set(false);
                        on_setup_finished();
                    }
                    Err(e) => {
                        set_step3_unlocking.set(false);
                        set_step3_error.set(Some(lang.get().translate_error(&e)));
                    }
                }
            });
        }
    };

    view! {
        <div class="wizard-container">
            <div class="wizard-header">
                <h2 class="wizard-title">
                    {move || lang.get().wizard_title()}
                </h2>
                <p class="wizard-subtitle">
                    {move || lang.get().wizard_subtitle()}
                </p>
            </div>

            // ==================== SEKME 1: Bulut Depolama ====================
            <div
                class=move || {
                    if current_step.get() == 1 {
                        "wizard-card wizard-card-active"
                    } else if current_step.get() > 1 {
                        "wizard-card wizard-card-done"
                    } else {
                        "wizard-card"
                    }
                }
                style=move || if current_step.get() == 1 { "display: block;" } else { "display: none;" }
            >
                <div class="wizard-step-header">
                    <span class="wizard-step-title">
                        {move || lang.get().wizard_step1_badge()}
                    </span>
                    {move || if drive_connected.get() {
                        view! { <span class="system-badge" style="color: var(--color-success);">"Connected"</span> }
                    } else {
                        view! { <span class="system-badge" style="color: var(--accent-color);">"Setup Needed"</span> }
                    }}
                </div>
                <div class="wizard-step-desc">
                    {move || lang.get().wizard_step1_desc()}
                </div>

                // If not yet connected
                {move || if !drive_connected.get() {
                    view! {
                        <div style="margin-top: 1rem;">
                            <button
                                class="btn btn-primary"
                                style="width: 100%; display: flex; align-items: center; justify-content: center; gap: 0.5rem;"
                                on:click=on_connect_drive
                                disabled=move || { step1_loading.get() || api_saving.get() }
                            >
                                {move || if step1_loading.get() {
                                    view! {
                                        <>
                                            <span class="spinner"></span>
                                            <span>{lang.get().wizard_connecting_drive()}</span>
                                        </>
                                    }
                                } else {
                                    view! {
                                        <>
                                            <span>{lang.get().wizard_connect_gdrive()}</span>
                                        </>
                                    }
                                }}
                            </button>

                            {move || step1_error.get().map(|err| {
                                view! {
                                    <div class="wizard-status-msg wizard-msg-error" style="margin-top: 0.75rem;">
                                        {lang.get().translate_error(&err)}
                                    </div>
                                }
                            })}
                        </div>
                    }.into_view()
                } else {
                    view! {
                        <div style="margin-top: 1rem;">
                            <div class="wizard-status-msg wizard-msg-success" style="margin-bottom: 1rem;">
                                {move || lang.get().wizard_step1_success()}
                            </div>
                            <button
                                class="btn btn-primary"
                                style="width: 100%;"
                                on:click=on_go_to_step2
                            >
                                <span>{move || lang.get().wizard_next_step_btn()}</span>
                            </button>
                        </div>
                    }.into_view()
                }}

                // Gelişmiş (Özel Google Drive API) Akordeon Bölmesi
                <div style="margin-top: 1rem;">
                    <button
                        type="button"
                        class="accordion-header-btn"
                        on:click=move |_| set_show_step1_advanced.update(|v| *v = !*v)
                    >
                        <span style="font-weight: 600; font-size: 0.88rem; display: flex; align-items: center;">
                            <span class=move || if show_step1_advanced.get() { "accordion-chevron open" } else { "accordion-chevron" }>
                                "▶"
                            </span>
                            {move || lang.get().wizard_advanced_api_expander()}
                        </span>
                    </button>

                    <div class=move || if show_step1_advanced.get() { "soft-collapse-wrapper expanded" } else { "soft-collapse-wrapper" }>
                        <div class="soft-collapse-inner">
                            <div style="padding: 0.85rem; background: rgba(0, 0, 0, 0.15); border: 1px solid var(--card-border); border-radius: var(--radius-box, 6px); display: flex; flex-direction: column; gap: 0.65rem;">
                                <p style="font-size: 0.8rem; color: var(--text-secondary); line-height: 1.4; margin: 0;">
                                    {move || lang.get().settings_custom_api_desc()}
                                </p>

                                <div class="wizard-input-group" style="margin-bottom: 0;">
                                    <label style="font-size: 0.8rem; font-weight: 600; color: var(--text-secondary);">
                                        {move || lang.get().settings_client_id_label()}
                                    </label>
                                    <input
                                        type="text"
                                        class="wizard-input selectable-text"
                                        placeholder="xxxx.apps.googleusercontent.com"
                                        prop:value=custom_client_id
                                        on:input=move |ev| set_custom_client_id.set(event_target_value(&ev))
                                        disabled=move || api_saving.get()
                                    />
                                </div>

                                <div class="wizard-input-group" style="margin-bottom: 0;">
                                    <label style="font-size: 0.8rem; font-weight: 600; color: var(--text-secondary);">
                                        {move || lang.get().settings_client_secret_label()}
                                    </label>
                                    <input
                                        type="password"
                                        class="wizard-input"
                                        placeholder="GOCSPX-xxxxxxxxxxxxxxxx"
                                        prop:value=custom_client_secret
                                        on:input=move |ev| set_custom_client_secret.set(event_target_value(&ev))
                                        disabled=move || api_saving.get()
                                    />
                                </div>

                                <button
                                    type="button"
                                    class="btn"
                                    style="width: 100%; display: flex; align-items: center; justify-content: center; gap: 0.4rem; margin-top: 0.25rem;"
                                    disabled=move || api_saving.get()
                                    on:click=move |_| {
                                        let cid = custom_client_id.get().trim().to_string();
                                        let csec = custom_client_secret.get().trim().to_string();
                                        set_api_saving.set(true);
                                        set_api_feedback_msg.set(None);
                                        leptos::spawn_local(async move {
                                            match tauri_bridge::set_drive_api_credentials(cid, csec).await {
                                                Ok(_) => {
                                                    set_api_saving.set(false);
                                                    set_api_feedback_msg.set(Some((true, lang.get().settings_api_saved_msg().to_string())));
                                                }
                                                Err(err) => {
                                                    set_api_saving.set(false);
                                                    set_api_feedback_msg.set(Some((false, err)));
                                                }
                                            }
                                        });
                                    }
                                >
                                    {move || if api_saving.get() {
                                        view! { <><span class="spinner"></span><span>"..."</span></> }.into_view()
                                    } else {
                                        view! { <span>{lang.get().settings_api_save_btn()}</span> }.into_view()
                                    }}
                                </button>

                                {move || api_feedback_msg.get().map(|(success, msg)| {
                                    let msg_class = if success { "wizard-status-msg wizard-msg-success" } else { "wizard-status-msg wizard-msg-error" };
                                    view! {
                                        <div class=msg_class style="margin-top: 0.35rem;">
                                            {msg}
                                        </div>
                                    }
                                })}
                            </div>
                        </div>
                    </div>
                </div>
            </div>

            // ==================== SEKME 2: Kriptografik Kasa Yapılandırması ====================
            <div
                class=move || {
                    if current_step.get() == 2 {
                        "wizard-card wizard-card-active"
                    } else if current_step.get() > 2 {
                        "wizard-card wizard-card-done"
                    } else {
                        "wizard-card"
                    }
                }
                style=move || if current_step.get() == 2 { "display: block;" } else { "display: none;" }
            >
                <div class="wizard-step-header">
                    <span class="wizard-step-title">
                        {move || lang.get().wizard_step2_badge()}
                    </span>
                </div>
                <div class="wizard-step-desc">
                    {move || lang.get().wizard_step2_desc()}
                </div>

                // Scanning spinner
                {move || if scanning_vaults.get() {
                    view! {
                        <div style="display: flex; align-items: center; justify-content: center; gap: 0.75rem; padding: 2rem 0; color: var(--text-secondary);">
                            <span class="spinner"></span>
                            <span>{lang.get().wizard_scanning_vaults()}</span>
                        </div>
                    }.into_view()
                } else if !remote_vaults.get().is_empty() {
                    // DURUM 1: Bulutta en az bir kasa bulundu!
                    view! {
                        <div style="margin-top: 1rem;">
                            // Başlık: Mevcut Kasa Tespit Edildi
                            <button
                                type="button"
                                class="accordion-header-btn"
                                on:click=move |_| set_show_create_new.set(false)
                            >
                                <span style="font-weight: 600; font-size: 0.88rem; display: flex; align-items: center;">
                                    <span class=move || if !show_create_new.get() { "accordion-chevron open" } else { "accordion-chevron" }>
                                        "▶"
                                    </span>
                                    {lang.get().wizard_existing_vault_title()}
                                </span>
                                <span class="system-badge" style="color: var(--color-success);">"Cloud Detected"</span>
                            </button>

                            // Gövde: Mevcut Kasa (Karşılıklı Soft Akordeon)
                            <div class=move || if !show_create_new.get() { "soft-collapse-wrapper expanded" } else { "soft-collapse-wrapper" }>
                                <div class="soft-collapse-inner">
                                    <div style="padding: 1rem; background: rgba(0, 0, 0, 0.18); border: 1px solid var(--border-color); border-radius: var(--radius-box, 6px); display: flex; flex-direction: column; gap: 0.75rem;">
                                        <p style="font-size: 0.8rem; color: var(--text-secondary); margin: 0; line-height: 1.4;">
                                            {lang.get().wizard_existing_vault_desc()}
                                        </p>

                                        // Kasa seçimi (tekse rozet, birden fazlaysa select)
                                        {move || {
                                            let vaults = remote_vaults.get();
                                            if vaults.len() > 1 {
                                                view! {
                                                    <div>
                                                        <select
                                                            class="wizard-input"
                                                            on:change=move |ev| {
                                                                let val = event_target_value(&ev);
                                                                set_selected_vault.set(val);
                                                                set_recovery_verified.set(false);
                                                                set_existing_vault_error.set(None);
                                                            }
                                                        >
                                                            {vaults.into_iter().map(|v| {
                                                                let v_clone = v.clone();
                                                                let is_sel = {
                                                                    let v_check = v_clone.clone();
                                                                    move || selected_vault.get() == v_check
                                                                };
                                                                view! { <option value=v_clone selected=is_sel>{v}</option> }
                                                            }).collect_view()}
                                                        </select>
                                                    </div>
                                                }.into_view()
                                            } else {
                                                view! {
                                                    <div style="padding: 0.5rem 0.75rem; background: rgba(255, 255, 255, 0.05); border-radius: 6px; font-weight: 600; font-size: 0.9rem;">
                                                        {format!("📁 {}", selected_vault.get())}
                                                    </div>
                                                }.into_view()
                                            }
                                        }}

                                        // Henüz kurtarma anahtarı doğrulanmadıysa
                                        {move || if !recovery_verified.get() {
                                            view! {
                                                <div style="display: flex; flex-direction: column; gap: 0.5rem;">
                                                    <input
                                                        type="text"
                                                        class="wizard-input selectable-text"
                                                        placeholder=move || lang.get().recovery_key_placeholder()
                                                        prop:value=recovery_key_input
                                                        on:input=move |ev| set_recovery_key_input.set(event_target_value(&ev))
                                                        disabled=move || recovery_verifying.get()
                                                    />
                                                    <button
                                                        class="btn btn-primary"
                                                        style="width: 100%; display: flex; align-items: center; justify-content: center; gap: 0.5rem;"
                                                        on:click=on_verify_recovery_key
                                                        disabled=move || recovery_verifying.get()
                                                    >
                                                        {move || if recovery_verifying.get() {
                                                            view! {
                                                                <>
                                                                    <span class="spinner"></span>
                                                                    <span>"..."</span>
                                                                </>
                                                            }
                                                        } else {
                                                            view! {
                                                                <>
                                                                    <span>{lang.get().wizard_verify_key_btn()}</span>
                                                                </>
                                                            }
                                                        }}
                                                    </button>
                                                </div>
                                            }.into_view()
                                        } else {
                                            // Kurtarma anahtarı doğrulandı! Yeni şifre alanları
                                            view! {
                                                <div style="display: flex; flex-direction: column; gap: 0.6rem;">
                                                    <div class="wizard-status-msg wizard-msg-success" style="font-size: 0.8rem; margin: 0 0 0.4rem 0;">
                                                        {lang.get().wizard_key_verified_msg()}
                                                    </div>
                                                    <input
                                                        type="password"
                                                        class="wizard-input"
                                                        placeholder=move || lang.get().new_password_placeholder()
                                                        prop:value=existing_new_pw
                                                        on:input=move |ev| set_existing_new_pw.set(event_target_value(&ev))
                                                        disabled=move || linking_loading.get()
                                                    />
                                                    <input
                                                        type="password"
                                                        class="wizard-input"
                                                        placeholder=move || lang.get().wizard_confirm_placeholder()
                                                        prop:value=existing_confirm_pw
                                                        on:input=move |ev| set_existing_confirm_pw.set(event_target_value(&ev))
                                                        disabled=move || linking_loading.get()
                                                    />
                                                    <button
                                                        class="btn btn-primary"
                                                        style="width: 100%; display: flex; align-items: center; justify-content: center; gap: 0.5rem;"
                                                        on:click=on_link_and_unlock_vault
                                                        disabled=move || linking_loading.get()
                                                    >
                                                        {move || if linking_loading.get() {
                                                            view! {
                                                                <>
                                                                    <span class="spinner"></span>
                                                                    <span>"..."</span>
                                                                </>
                                                            }
                                                        } else {
                                                            view! {
                                                                <>
                                                                    <span>{lang.get().wizard_link_vault_btn()}</span>
                                                                </>
                                                            }
                                                        }}
                                                    </button>
                                                </div>
                                            }.into_view()
                                        }}

                                        {move || existing_vault_error.get().map(|err| {
                                            view! {
                                                <div class="wizard-status-msg wizard-msg-error" style="margin-top: 0.5rem;">
                                                    {lang.get().translate_error(&err)}
                                                </div>
                                            }
                                        })}
                                    </div>
                                </div>
                            </div>

                            // Alt kısım: Genişletilebilir "Yeni Kasa Oluştur" bölmesi
                            <div style="margin-top: 0.75rem;">
                                <button
                                    type="button"
                                    class="accordion-header-btn"
                                    on:click=move |_| set_show_create_new.set(true)
                                >
                                    <span style="font-weight: 600; font-size: 0.88rem; display: flex; align-items: center;">
                                        <span class=move || if show_create_new.get() { "accordion-chevron open" } else { "accordion-chevron" }>
                                            "▶"
                                        </span>
                                        {move || lang.get().wizard_create_new_expander()}
                                    </span>
                                </button>

                                <div class=move || if show_create_new.get() { "soft-collapse-wrapper expanded" } else { "soft-collapse-wrapper" }>
                                    <div class="soft-collapse-inner">
                                        <div style="padding: 1rem; background: rgba(0, 0, 0, 0.18); border: 1px solid var(--border-color); border-radius: var(--radius-box, 6px);">
                                            <div class="wizard-input-group">
                                                <input
                                                    type="text"
                                                    class="wizard-input"
                                                    placeholder=move || lang.get().wizard_vault_name_placeholder()
                                                    prop:value=new_vault_name
                                                    on:input=move |ev| set_new_vault_name.set(event_target_value(&ev))
                                                    disabled=move || step2_create_loading.get()
                                                />
                                                <input
                                                    type="password"
                                                    class="wizard-input"
                                                    placeholder=move || lang.get().wizard_password_placeholder()
                                                    prop:value=password
                                                    on:input=move |ev| set_password.set(event_target_value(&ev))
                                                    disabled=move || step2_create_loading.get()
                                                />
                                                <input
                                                    type="password"
                                                    class="wizard-input"
                                                    placeholder=move || lang.get().wizard_confirm_placeholder()
                                                    prop:value=confirm_password
                                                    on:input=move |ev| set_confirm_password.set(event_target_value(&ev))
                                                    disabled=move || step2_create_loading.get()
                                                />
                                            </div>

                                            <button
                                                class="btn btn-primary"
                                                style="width: 100%;"
                                                on:click=on_create_new_vault
                                                disabled=move || step2_create_loading.get()
                                            >
                                                {move || if step2_create_loading.get() {
                                                    view! {
                                                        <>
                                                            <span class="spinner"></span>
                                                            <span>{lang.get().wizard_initializing()}</span>
                                                        </>
                                                    }
                                                } else {
                                                    view! {
                                                        <>
                                                            <span>{lang.get().wizard_init_vault()}</span>
                                                        </>
                                                    }
                                                }}
                                            </button>

                                            {move || step2_create_error.get().map(|err| {
                                                view! {
                                                    <div class="wizard-status-msg wizard-msg-error" style="margin-top: 0.5rem;">
                                                        {lang.get().translate_error(&err)}
                                                    </div>
                                                }
                                            })}
                                        </div>
                                    </div>
                                </div>
                            </div>
                        </div>
                    }.into_view()
                } else {
                    // DURUM 2: Bulutta hiç kasa yoksa -> Doğrudan Yeni Kasa Oluşturma Formu
                    view! {
                        <div style="margin-top: 1rem;">
                            <div class="wizard-input-group">
                                <input
                                    type="text"
                                    class="wizard-input"
                                    placeholder=move || lang.get().wizard_vault_name_placeholder()
                                    prop:value=new_vault_name
                                    on:input=move |ev| set_new_vault_name.set(event_target_value(&ev))
                                    disabled=move || step2_create_loading.get()
                                />
                                <input
                                    type="password"
                                    class="wizard-input"
                                    placeholder=move || lang.get().wizard_password_placeholder()
                                    prop:value=password
                                    on:input=move |ev| set_password.set(event_target_value(&ev))
                                    disabled=move || step2_create_loading.get()
                                />
                                <input
                                    type="password"
                                    class="wizard-input"
                                    placeholder=move || lang.get().wizard_confirm_placeholder()
                                    prop:value=confirm_password
                                    on:input=move |ev| set_confirm_password.set(event_target_value(&ev))
                                    disabled=move || step2_create_loading.get()
                                />
                            </div>

                            <button
                                class="btn btn-primary"
                                style="width: 100%;"
                                on:click=on_create_new_vault
                                disabled=move || step2_create_loading.get()
                            >
                                {move || if step2_create_loading.get() {
                                    view! {
                                        <>
                                            <span class="spinner"></span>
                                            <span>{lang.get().wizard_initializing()}</span>
                                        </>
                                    }
                                } else {
                                    view! {
                                        <>
                                            <span>{lang.get().wizard_init_vault()}</span>
                                        </>
                                    }
                                }}
                            </button>

                            {move || step2_create_error.get().map(|err| {
                                view! {
                                    <div class="wizard-status-msg wizard-msg-error" style="margin-top: 0.5rem;">
                                        {lang.get().translate_error(&err)}
                                    </div>
                                }
                            })}
                        </div>
                    }.into_view()
                }}
            </div>

            // ==================== SEKME 3: Kurtarma Anahtarını Yedekle ====================
            <div
                class=move || {
                    if current_step.get() == 3 {
                        "wizard-card wizard-card-active"
                    } else {
                        "wizard-card"
                    }
                }
                style=move || if current_step.get() == 3 { "display: block;" } else { "display: none;" }
            >
                <div class="wizard-step-header">
                    <span class="wizard-step-title">
                        {move || lang.get().wizard_step3_badge()}
                    </span>
                </div>

                <div class="recovery-key-box" style="margin-top: 10px; padding: 14px; background: rgba(0, 0, 0, 0.2); border: 1px solid var(--accent-color); border-radius: 8px;">
                    <h4 style="margin: 0 0 6px 0; color: var(--accent-color); font-size: 0.95rem;">
                        {move || lang.get().wizard_recovery_title()}
                    </h4>
                    <p style="font-size: 0.82rem; margin: 0 0 10px 0; color: var(--text-secondary); line-height: 1.4;">
                        {move || lang.get().wizard_recovery_desc()}
                    </p>
                    <div style="padding: 10px; background: rgba(0, 0, 0, 0.35); border-radius: 6px; font-family: monospace; font-size: 0.95rem; word-break: break-all; user-select: all; text-align: center; margin-bottom: 12px; border: 1px dashed var(--accent-color); font-weight: 700; letter-spacing: 1px;">
                        <code class="selectable-text">{move || generated_recovery_key.get()}</code>
                    </div>

                    // Kopyalama Butonu
                    <button
                        type="button"
                        class="btn"
                        style="width: 100%; margin-bottom: 0.65rem; display: flex; align-items: center; justify-content: center; gap: 0.45rem;"
                        on:click=move |_| {
                            let k = generated_recovery_key.get();
                            copy_text_to_clipboard(&k);
                            set_key_copied.set(true);
                            leptos::spawn_local(async move {
                                gloo_timers::future::TimeoutFuture::new(2000).await;
                                set_key_copied.set(false);
                            });
                        }
                    >
                        {move || if key_copied.get() {
                            view! { <span style="color: var(--color-success); font-weight: 600;">{lang.get().wizard_key_copied_btn()}</span> }.into_view()
                        } else {
                            view! { <span>"📋 " {lang.get().wizard_copy_key_btn()}</span> }.into_view()
                        }}
                    </button>

                    <button
                        class="btn btn-primary"
                        style=move || {
                            if recovery_countdown.get() > 0 || step3_unlocking.get() {
                                "width: 100%; opacity: 0.65; cursor: not-allowed;"
                            } else {
                                "width: 100%;"
                            }
                        }
                        disabled=move || { recovery_countdown.get() > 0 || step3_unlocking.get() }
                        on:click=on_finish_step3
                    >
                        {move || {
                            let rem = recovery_countdown.get();
                            if rem > 0 {
                                lang.get().wizard_recovery_countdown_btn(rem)
                            } else if step3_unlocking.get() {
                                lang.get().wizard_initializing().to_string()
                            } else {
                                lang.get().wizard_recovery_confirm_btn().to_string()
                            }
                        }}
                    </button>

                    {move || step3_error.get().map(|err| {
                        view! {
                            <div class="wizard-status-msg wizard-msg-error" style="margin-top: 0.6rem;">
                                {err}
                            </div>
                        }
                    })}
                </div>
            </div>
        </div>
    }
}
