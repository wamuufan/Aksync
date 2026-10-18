use crate::i18n::Language;
use crate::tauri_bridge;
use leptos::*;

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
    let (step1_loading, set_step1_loading) = create_signal(false);
    let (step1_error, set_step1_error) = create_signal(Option::<String>::None);

    let (step2_loading, set_step2_loading) = create_signal(false);
    let (step2_error, set_step2_error) = create_signal(Option::<String>::None);
    let (generated_recovery_key, set_generated_recovery_key) = create_signal(Option::<String>::None);

    let on_connect_drive = move |_| {
        set_step1_loading.set(true);
        set_step1_error.set(None);
        leptos::spawn_local(async move {
            match tauri_bridge::trigger_drive_auth().await {
                Ok(_) => {
                    set_step1_loading.set(false);
                    set_step1_done.set(true);
                }
                Err(err) => {
                    set_step1_loading.set(false);
                    set_step1_error.set(Some(err));
                }
            }
        });
    };

    let on_init_vault = move |_| {
        let pw = password.get();
        let confirm = confirm_password.get();

        if pw.len() < 8 {
            set_step2_error.set(Some(lang.get().wizard_pw_short().to_string()));
            return;
        }

        if pw != confirm {
            set_step2_error.set(Some(lang.get().wizard_pw_mismatch().to_string()));
            return;
        }

        set_step2_loading.set(true);
        set_step2_error.set(None);

        leptos::spawn_local(async move {
            match tauri_bridge::initialize_vault(pw).await {
                Ok(msg) => {
                    set_step2_loading.set(false);
                    if let Some(pos) = msg.find("Recovery Key:") {
                        let key = msg[pos + "Recovery Key:".len()..].trim().to_string();
                        set_generated_recovery_key.set(Some(key));
                    } else {
                        on_setup_finished();
                    }
                }
                Err(err) => {
                    set_step2_loading.set(false);
                    set_step2_error.set(Some(err));
                }
            }
        });
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

            // Step 1: Google Drive OAuth
            <div class=move || {
                if step1_done.get() {
                    "wizard-card wizard-card-done"
                } else {
                    "wizard-card wizard-card-active"
                }
            }>
                <div class="wizard-step-header">
                    <span class="wizard-step-title">
                        {move || lang.get().wizard_step1_badge()}
                    </span>
                    {move || if step1_done.get() {
                        view! { <span class="system-badge" style="color: var(--color-success);">"Ready"</span> }
                    } else {
                        view! { <span class="system-badge" style="color: var(--accent-color);">"Setup Needed"</span> }
                    }}
                </div>
                <div class="wizard-step-desc">
                    {move || lang.get().wizard_step1_desc()}
                </div>

                // Keep DOM elements persistent instead of unmounting conditionally
                <div style=move || if step1_done.get() { "display: none;" } else { "display: block;" }>
                    <button
                        class="btn btn-primary"
                        on:click=on_connect_drive
                        disabled=move || step1_loading.get()
                    >
                        {move || if step1_loading.get() {
                            view! {
                                <>
                                    <span class="spinner"></span>
                                    <span>{lang.get().wizard_connecting()}</span>
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
                            <div class="wizard-status-msg wizard-msg-error">
                                {lang.get().translate_error(&err)}
                            </div>
                        }
                    })}
                </div>

                <div
                    class="wizard-status-msg wizard-msg-success"
                    style=move || if step1_done.get() { "display: block;" } else { "display: none;" }
                >
                    {move || lang.get().wizard_step1_success()}
                </div>
            </div>

            // Step 2: Cryptographic Vault Password
            <div class="wizard-card">
                <div class="wizard-step-header">
                    <span class="wizard-step-title">
                        {move || lang.get().wizard_step2_badge()}
                    </span>
                </div>
                <div class="wizard-step-desc">
                    {move || lang.get().wizard_step2_desc()}
                </div>

                // Keep inputs unconditionally mounted in DOM to prevent losing focus/value on reactivity triggers
                <div style=move || if step1_done.get() { "display: block;" } else { "display: none;" }>
                    {move || if let Some(key) = generated_recovery_key.get() {
                        view! {
                            <div class="recovery-key-box" style="margin-top: 10px; padding: 14px; background: rgba(0, 0, 0, 0.2); border: 1px solid var(--accent-color); border-radius: 8px;">
                                <h4 style="margin: 0 0 6px 0; color: var(--accent-color); font-size: 0.95rem;">
                                    {lang.get().wizard_recovery_title()}
                                </h4>
                                <p style="font-size: 0.82rem; margin: 0 0 10px 0; color: var(--text-secondary); line-height: 1.4;">
                                    {lang.get().wizard_recovery_desc()}
                                </p>
                                <div style="padding: 10px; background: rgba(0, 0, 0, 0.35); border-radius: 6px; font-family: monospace; font-size: 0.95rem; word-break: break-all; user-select: all; text-align: center; margin-bottom: 12px; border: 1px dashed var(--accent-color); font-weight: 700; letter-spacing: 1px;">
                                    <code>{key}</code>
                                </div>
                                <button
                                    class="btn btn-primary"
                                    style="width: 100%;"
                                    on:click=move |_| on_setup_finished()
                                >
                                    {lang.get().wizard_recovery_confirm_btn()}
                                </button>
                            </div>
                        }.into_view()
                    } else {
                        view! {
                            <>
                                <div class="wizard-input-group">
                                    <input
                                        type="password"
                                        class="wizard-input"
                                        placeholder=move || lang.get().wizard_password_placeholder()
                                        prop:value=password
                                        on:input=move |ev| set_password.set(event_target_value(&ev))
                                        disabled=move || step2_loading.get()
                                    />
                                    <input
                                        type="password"
                                        class="wizard-input"
                                        placeholder=move || lang.get().wizard_confirm_placeholder()
                                        prop:value=confirm_password
                                        on:input=move |ev| set_confirm_password.set(event_target_value(&ev))
                                        disabled=move || step2_loading.get()
                                    />
                                </div>

                                <button
                                    class="btn btn-primary"
                                    on:click=on_init_vault
                                    disabled=move || step2_loading.get()
                                >
                                    {move || if step2_loading.get() {
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

                                {move || step2_error.get().map(|err| {
                                    view! {
                                        <div class="wizard-status-msg wizard-msg-error">
                                            {lang.get().translate_error(&err)}
                                        </div>
                                    }
                                })}
                            </>
                        }.into_view()
                    }}
                </div>

                <div
                    style=move || if step1_done.get() { "display: none;" } else { "display: block;" }
                    class="wizard-step-desc"
                    style:font-style="italic"
                >
                    "Complete Step 1 to unlock Vault Setup."
                </div>
            </div>
        </div>
    }
}
