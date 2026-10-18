#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    English,
    Turkish,
}

#[allow(dead_code)]
impl Language {
    pub fn toggle(&self) -> Self {
        match self {
            Self::English => Self::Turkish,
            Self::Turkish => Self::English,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            Self::English => "EN",
            Self::Turkish => "TR",
        }
    }

    pub fn brand_subtitle(&self) -> &'static str {
        match self {
            Self::English => "Files On-Demand",
            Self::Turkish => "İsteğe Bağlı Dosyalar",
        }
    }

    pub fn e2ee_badge(&self) -> &'static str {
        "E2EE Active"
    }

    pub fn sync_status_title(&self) -> &'static str {
        match self {
            Self::English => "Synchronization Status",
            Self::Turkish => "Eşitleme Durumu",
        }
    }

    pub fn state_setup_required(&self) -> &'static str {
        match self {
            Self::English => "Setup Required",
            Self::Turkish => "Kurulum Gerekli",
        }
    }

    pub fn state_connecting(&self) -> &'static str {
        match self {
            Self::English => "Connecting to cloud...",
            Self::Turkish => "Buluta bağlanıyor...",
        }
    }

    pub fn state_idle(&self) -> &'static str {
        match self {
            Self::English => "All files up to date",
            Self::Turkish => "Tüm dosyalar güncel",
        }
    }

    pub fn state_syncing(&self) -> &'static str {
        match self {
            Self::English => "Syncing in progress...",
            Self::Turkish => "Eşitleme devam ediyor...",
        }
    }

    pub fn state_paused(&self) -> &'static str {
        match self {
            Self::English => "Sync Paused",
            Self::Turkish => "Eşitleme Duraklatıldı",
        }
    }

    pub fn state_locked(&self) -> &'static str {
        match self {
            Self::English => "Vault Locked",
            Self::Turkish => "Kasa Kilitli",
        }
    }

    pub fn state_offline(&self) -> &'static str {
        match self {
            Self::English => "Daemon Offline",
            Self::Turkish => "Daemon Çevrimdışı",
        }
    }

    pub fn state_error(&self) -> &'static str {
        match self {
            Self::English => "Error",
            Self::Turkish => "Hata",
        }
    }

    pub fn vault_locked_title(&self) -> &'static str {
        match self {
            Self::English => "Unlock Encrypted Vault",
            Self::Turkish => "Şifreli Kasayı Aç",
        }
    }

    pub fn vault_locked_subtitle(&self) -> &'static str {
        match self {
            Self::English => {
                "Enter your master vault password to mount your zero-knowledge cloud drive."
            }
            Self::Turkish => "Sıfır-bilgi bulut sürücünüzü bağlamak için ana kasa şifrenizi girin.",
        }
    }

    pub fn unlock_btn(&self) -> &'static str {
        match self {
            Self::English => "Unlock Vault",
            Self::Turkish => "Kasayı Aç",
        }
    }

    pub fn unlocking_btn(&self) -> &'static str {
        match self {
            Self::English => "Unlocking & Mounting...",
            Self::Turkish => "Açılıyor ve Bağlanıyor...",
        }
    }

    pub fn unlock_placeholder(&self) -> &'static str {
        match self {
            Self::English => "Enter Master Password...",
            Self::Turkish => "Ana Şifreyi Girin...",
        }
    }

    pub fn upload_speed(&self) -> &'static str {
        match self {
            Self::English => "Upload Speed",
            Self::Turkish => "Yükleme Hızı",
        }
    }

    pub fn download_speed(&self) -> &'static str {
        match self {
            Self::English => "Download Speed",
            Self::Turkish => "İndirme Hızı",
        }
    }

    pub fn storage_usage(&self) -> &'static str {
        match self {
            Self::English => "Cloud Storage",
            Self::Turkish => "Bulut Depolama",
        }
    }

    pub fn files_synced(&self) -> &'static str {
        match self {
            Self::English => "Monitored Files",
            Self::Turkish => "İzlenen Dosyalar",
        }
    }

    pub fn pause_btn(&self) -> &'static str {
        match self {
            Self::English => "Pause Sync",
            Self::Turkish => "Eşitlemeyi Duraklat",
        }
    }

    pub fn resume_btn(&self) -> &'static str {
        match self {
            Self::English => "Resume Sync",
            Self::Turkish => "Eşitlemeyi Sürdür",
        }
    }

    pub fn sync_now_btn(&self) -> &'static str {
        match self {
            Self::English => "Sync Now",
            Self::Turkish => "Şimdi Eşitle",
        }
    }

    pub fn open_folder_btn(&self) -> &'static str {
        match self {
            Self::English => "Open Folder",
            Self::Turkish => "Klasörü Aç",
        }
    }

    pub fn system_details(&self) -> &'static str {
        match self {
            Self::English => "Mount & Security Parameters",
            Self::Turkish => "Bağlantı ve Güvenlik Parametreleri",
        }
    }

    pub fn cipher(&self) -> &'static str {
        match self {
            Self::English => "Cipher Suite",
            Self::Turkish => "Şifreleme Algoritması",
        }
    }

    pub fn kdf(&self) -> &'static str {
        match self {
            Self::English => "Key Derivation",
            Self::Turkish => "Anahtar Türetme",
        }
    }

    pub fn mount_point(&self) -> &'static str {
        match self {
            Self::English => "Target FUSE Mount",
            Self::Turkish => "Hedef FUSE Bağlantı Noktası",
        }
    }

    pub fn cache_path(&self) -> &'static str {
        match self {
            Self::English => "Encrypted Cache",
            Self::Turkish => "Şifreli Önbellek",
        }
    }

    pub fn sync_folder(&self) -> &'static str {
        match self {
            Self::English => "Local Sync Folder",
            Self::Turkish => "Yerel Eşitleme Klasörü",
        }
    }

    pub fn connected_account(&self) -> &'static str {
        match self {
            Self::English => "Google Drive (Encrypted)",
            Self::Turkish => "Google Drive (Şifrelenmiş)",
        }
    }

    pub fn open_btn_short(&self) -> &'static str {
        match self {
            Self::English => "Open",
            Self::Turkish => "Aç",
        }
    }

    pub fn nav_sync_folders(&self) -> &'static str {
        match self {
            Self::English => "Sync & Folders",
            Self::Turkish => "Eşitleme ve Klasörler",
        }
    }

    pub fn nav_vault_security(&self) -> &'static str {
        match self {
            Self::English => "Vault Security",
            Self::Turkish => "Kasa Güvenliği",
        }
    }

    pub fn nav_app_settings(&self) -> &'static str {
        match self {
            Self::English => "Application Settings",
            Self::Turkish => "Uygulama Ayarları",
        }
    }

    pub fn synced_home_folders_title(&self) -> &'static str {
        match self {
            Self::English => "Synced Home Folders",
            Self::Turkish => "Eşitlenen Ev Klasörleri",
        }
    }

    pub fn synced_home_folders_subtitle(&self) -> &'static str {
        match self {
            Self::English => "Selected folders are automatically backed up to the encrypted vault",
            Self::Turkish => "Seçili klasörler şifreli kasaya otomatik yedeklenir",
        }
    }

    pub fn sync_hidden_checkbox_label(&self) -> &'static str {
        match self {
            Self::English => "Sync hidden files (starting with .)",
            Self::Turkish => "Gizli dosyaları (. ile başlayanlar) eşitle",
        }
    }

    pub fn storage_short_label(&self) -> &'static str {
        match self {
            Self::English => "Storage",
            Self::Turkish => "Depolama",
        }
    }

    pub fn pause_short_btn(&self) -> &'static str {
        match self {
            Self::English => "Pause",
            Self::Turkish => "Duraklat",
        }
    }

    pub fn resume_short_btn(&self) -> &'static str {
        match self {
            Self::English => "Resume",
            Self::Turkish => "Sürdür",
        }
    }

    pub fn sync_folders_title(&self) -> &'static str {
        match self {
            Self::English => "Home Folders Sync",
            Self::Turkish => "Home Klasör Eşitleme",
        }
    }

    pub fn sync_folders_desc(&self) -> &'static str {
        match self {
            Self::English => "Select user home folders to sync with Google Drive",
            Self::Turkish => "Google Drive'a eşitlenecek kullanıcı klasörlerini seçin",
        }
    }

    pub fn sync_folder_target_hint(&self) -> &'static str {
        match self {
            Self::English => "Target: Encrypted Vault /home/",
            Self::Turkish => "Hedef: Şifreli Kasa /home/",
        }
    }

    pub fn sync_folders_empty(&self) -> &'static str {
        match self {
            Self::English => "No folders detected in home directory",
            Self::Turkish => "Home dizininde klasör bulunamadı",
        }
    }

    // --- Setup Wizard Translations ---

    pub fn wizard_title(&self) -> &'static str {
        match self {
            Self::English => "Welcome to MyCloud",
            Self::Turkish => "MyCloud'a Hoş Geldiniz",
        }
    }

    pub fn wizard_subtitle(&self) -> &'static str {
        match self {
            Self::English => "First-Time Setup: Configure your cloud storage and encryption vault.",
            Self::Turkish => "İlk Kurulum: Bulut depolamanızı ve şifreleme kasanızı yapılandırın.",
        }
    }

    pub fn wizard_step1_badge(&self) -> &'static str {
        match self {
            Self::English => "Step 1: Cloud Storage",
            Self::Turkish => "1. Adım: Bulut Depolama",
        }
    }

    pub fn wizard_step1_desc(&self) -> &'static str {
        match self {
            Self::English => "Connect your Google Drive account using secure OAuth2 authorization.",
            Self::Turkish => "Güvenli OAuth2 yetkilendirmesi ile Google Drive hesabınızı bağlayın.",
        }
    }

    pub fn wizard_connect_gdrive(&self) -> &'static str {
        match self {
            Self::English => "Connect Google Drive",
            Self::Turkish => "Google Drive'a Bağlan",
        }
    }

    pub fn wizard_connecting(&self) -> &'static str {
        match self {
            Self::English => "Authorizing via Browser...",
            Self::Turkish => "Tarayıcı ile Yetkilendiriliyor...",
        }
    }

    pub fn wizard_step1_success(&self) -> &'static str {
        match self {
            Self::English => "Google Drive connected successfully!",
            Self::Turkish => "Google Drive başarıyla bağlandı!",
        }
    }

    pub fn wizard_step2_badge(&self) -> &'static str {
        match self {
            Self::English => "Step 2: Cryptographic Vault",
            Self::Turkish => "2. Adım: Kriptografik Kasa",
        }
    }

    pub fn wizard_step2_desc(&self) -> &'static str {
        match self {
            Self::English => "Set your Master Password. It derives an Argon2id key to encrypt all files before uploading.",
            Self::Turkish => "Ana Şifrenizi belirleyin. Yükleme öncesinde tüm dosyaları şifrelemek için Argon2id anahtarı türetilir.",
        }
    }

    pub fn wizard_password_placeholder(&self) -> &'static str {
        match self {
            Self::English => "Enter Master Password (min 8 chars)",
            Self::Turkish => "Ana Şifrenizi Girin (en az 8 karakter)",
        }
    }

    pub fn wizard_confirm_placeholder(&self) -> &'static str {
        match self {
            Self::English => "Confirm Master Password",
            Self::Turkish => "Ana Şifreyi Doğrulayın",
        }
    }

    pub fn wizard_init_vault(&self) -> &'static str {
        match self {
            Self::English => "Initialize Vault & Launch",
            Self::Turkish => "Kasayı Başlat ve Aç",
        }
    }

    pub fn wizard_initializing(&self) -> &'static str {
        match self {
            Self::English => "Deriving Argon2id Key...",
            Self::Turkish => "Argon2id Anahtarı Türetiliyor...",
        }
    }

    pub fn wizard_pw_mismatch(&self) -> &'static str {
        match self {
            Self::English => "Passwords do not match!",
            Self::Turkish => "Şifreler uyuşmuyor!",
        }
    }

    pub fn wizard_pw_short(&self) -> &'static str {
        match self {
            Self::English => "Password must be at least 8 characters.",
            Self::Turkish => "Şifre en az 8 karakter olmalıdır.",
        }
    }

    pub fn settings_btn(&self) -> &'static str {
        match self {
            Self::English => "Settings",
            Self::Turkish => "Ayarlar",
        }
    }

    pub fn settings_title(&self) -> &'static str {
        match self {
            Self::English => "Preferences",
            Self::Turkish => "Ayarlar",
        }
    }

    pub fn settings_appearance_section(&self) -> &'static str {
        match self {
            Self::English => "Appearance",
            Self::Turkish => "Görünüm",
        }
    }

    pub fn settings_theme_mode(&self) -> &'static str {
        match self {
            Self::English => "Theme Style",
            Self::Turkish => "Tema Modu",
        }
    }

    pub fn settings_ui_scale(&self) -> &'static str {
        match self {
            Self::English => "Interface Scale",
            Self::Turkish => "Arayüz Ölçeği",
        }
    }

    pub fn settings_ui_scale_desc(&self) -> &'static str {
        match self {
            Self::English => "Adjust text and widget sizing (Ctrl + / Ctrl - / Ctrl 0)",
            Self::Turkish => "Metin ve bileşen boyutlandırması (Ctrl + / Ctrl - / Ctrl 0)",
        }
    }

    pub fn theme_system(&self) -> &'static str {
        match self {
            Self::English => "System (Auto)",
            Self::Turkish => "Sistem (Otomatik)",
        }
    }

    pub fn theme_light(&self) -> &'static str {
        match self {
            Self::English => "Light",
            Self::Turkish => "Açık",
        }
    }

    pub fn theme_dark(&self) -> &'static str {
        match self {
            Self::English => "Dark",
            Self::Turkish => "Koyu",
        }
    }

    pub fn settings_accent_color(&self) -> &'static str {
        match self {
            Self::English => "Accent Color",
            Self::Turkish => "Vurgu Rengi",
        }
    }

    pub fn accent_system_dynamic(&self) -> &'static str {
        match self {
            Self::English => "System Dynamic (GNOME / Plasma)",
            Self::Turkish => "Sistem Dinamik (GNOME / Plasma)",
        }
    }

    pub fn settings_desktop_env(&self) -> &'static str {
        match self {
            Self::English => "Detected Desktop",
            Self::Turkish => "Algılanan Masaüstü",
        }
    }

    pub fn settings_fallback_badge(&self) -> &'static str {
        match self {
            Self::English => "Standard Adwaita Fallback",
            Self::Turkish => "Standart Adwaita Güvenli Mod",
        }
    }

    pub fn settings_advanced_theme(&self) -> &'static str {
        match self {
            Self::English => "Advanced Theme",
            Self::Turkish => "Gelişmiş Tema",
        }
    }

    pub fn settings_custom_theme_desc(&self) -> &'static str {
        match self {
            Self::English => "Override system colors manually",
            Self::Turkish => "Sistem renklerini el ile özelleştirin",
        }
    }

    pub fn settings_custom_enable(&self) -> &'static str {
        match self {
            Self::English => "Custom Colors",
            Self::Turkish => "Özel Renkler",
        }
    }

    pub fn settings_custom_window_bg(&self) -> &'static str {
        match self {
            Self::English => "Window Background",
            Self::Turkish => "Pencere Arka Planı",
        }
    }

    pub fn settings_custom_card_bg(&self) -> &'static str {
        match self {
            Self::English => "Card / Panel Color",
            Self::Turkish => "Kart ve Panel Rengi",
        }
    }

    pub fn settings_custom_accent(&self) -> &'static str {
        match self {
            Self::English => "Accent Color",
            Self::Turkish => "Vurgu Rengi",
        }
    }

    pub fn settings_custom_text(&self) -> &'static str {
        match self {
            Self::English => "Text Color",
            Self::Turkish => "Metin Rengi",
        }
    }

    pub fn settings_custom_reset(&self) -> &'static str {
        match self {
            Self::English => "Reset to System Colors",
            Self::Turkish => "Sistem Renklerine Sıfırla",
        }
    }

    pub fn settings_general_section(&self) -> &'static str {
        match self {
            Self::English => "General Settings",
            Self::Turkish => "Genel Ayarlar",
        }
    }

    pub fn settings_language(&self) -> &'static str {
        match self {
            Self::English => "Interface Language",
            Self::Turkish => "Arayüz Dili",
        }
    }

    pub fn settings_startup_launch(&self) -> &'static str {
        match self {
            Self::English => "Start minimized at login",
            Self::Turkish => "Girişte simge durumunda başlat",
        }
    }

    pub fn settings_sync_hidden_files(&self) -> &'static str {
        match self {
            Self::English => "Sync Hidden Files",
            Self::Turkish => "Gizli Dosyaları Eşitle",
        }
    }

    pub fn settings_sync_hidden_files_desc(&self) -> &'static str {
        match self {
            Self::English => "Upload files and directories starting with a dot (e.g. .directory, .git)",
            Self::Turkish => "Nokta ile başlayan gizli dosya ve klasörleri (.directory vb.) kasaya yükle",
        }
    }

    pub fn settings_advanced_section(&self) -> &'static str {
        match self {
            Self::English => "Advanced",
            Self::Turkish => "Gelişmiş",
        }
    }

    pub fn settings_advanced_desc(&self) -> &'static str {
        match self {
            Self::English => "Custom Google Drive API & Developer Settings",
            Self::Turkish => "Özel Google Drive API ve Geliştirici Ayarları",
        }
    }

    pub fn settings_custom_api_title(&self) -> &'static str {
        match self {
            Self::English => "Custom Google Drive API",
            Self::Turkish => "Özel Google Drive API",
        }
    }

    pub fn settings_custom_api_desc(&self) -> &'static str {
        match self {
            Self::English => "To avoid Google Drive's shared rate limits (Error 403: Rate Limit Exceeded), enter your own OAuth Client ID & Secret from Google Cloud Console.",
            Self::Turkish => "Google Drive'ın genel paylaşılan kota sınırlarını (Hata 403: Rate Limit Exceeded) aşmak için Google Cloud Console'dan aldığınız kendi OAuth Client ID ve Secret bilgilerinizi girin.",
        }
    }

    pub fn settings_client_id_label(&self) -> &'static str {
        match self {
            Self::English => "Client ID",
            Self::Turkish => "Client ID (İstemci Kimliği)",
        }
    }

    pub fn settings_client_secret_label(&self) -> &'static str {
        match self {
            Self::English => "Client Secret",
            Self::Turkish => "Client Secret (Gizli Anahtar)",
        }
    }

    pub fn settings_api_save_btn(&self) -> &'static str {
        match self {
            Self::English => "Save",
            Self::Turkish => "Kaydet",
        }
    }

    pub fn settings_api_saved_msg(&self) -> &'static str {
        match self {
            Self::English => "API credentials saved successfully.",
            Self::Turkish => "API kimlikleri başarıyla kaydedildi.",
        }
    }

    pub fn settings_api_auth_btn(&self) -> &'static str {
        match self {
            Self::English => "Authorize with Google",
            Self::Turkish => "Google ile Yetkilendir",
        }
    }

    pub fn settings_api_authorizing_msg(&self) -> &'static str {
        match self {
            Self::English => "Opening browser for authorization...",
            Self::Turkish => "Yetkilendirme için tarayıcı açılıyor...",
        }
    }

    pub fn settings_api_auth_success_msg(&self) -> &'static str {
        match self {
            Self::English => "Google Drive authorized successfully!",
            Self::Turkish => "Google Drive yetkilendirmesi başarıyla tamamlandı!",
        }
    }

    pub fn settings_close_btn(&self) -> &'static str {
        match self {
            Self::English => "Done",
            Self::Turkish => "Tamam",
        }
    }

    pub fn wizard_recovery_title(&self) -> &'static str {
        match self {
            Self::English => "Save Your Recovery Key",
            Self::Turkish => "Kurtarma Anahtarınızı Kaydedin",
        }
    }

    pub fn wizard_recovery_desc(&self) -> &'static str {
        match self {
            Self::English => "This 52-character key is the ONLY way to recover your vault if you forget your master password. Store it safely:",
            Self::Turkish => "Ana şifrenizi unutursanız kasanızı kurtarmak için tek yol bu 52 karakterlik anahtardır. Güvenli bir yere kaydedin:",
        }
    }

    pub fn wizard_recovery_confirm_btn(&self) -> &'static str {
        match self {
            Self::English => "I Saved the Key, Launch Vault",
            Self::Turkish => "Anahtarı Kaydettim, Kasayı Aç",
        }
    }

    pub fn lock_vault_btn(&self) -> &'static str {
        match self {
            Self::English => "Lock Vault",
            Self::Turkish => "Kasayı Kilitle",
        }
    }

    pub fn forgot_password_link(&self) -> &'static str {
        match self {
            Self::English => "Forgot Password? (Use Recovery Key)",
            Self::Turkish => "Şifremi Unuttum (Kurtarma Anahtarı Kullan)",
        }
    }

    pub fn back_to_unlock_link(&self) -> &'static str {
        match self {
            Self::English => "Back to Password Unlock",
            Self::Turkish => "Şifre ile Açmaya Dön",
        }
    }

    pub fn recovery_key_placeholder(&self) -> &'static str {
        match self {
            Self::English => "Enter 52-Character Recovery Key...",
            Self::Turkish => "52 Karakterlik Kurtarma Anahtarını Girin...",
        }
    }

    pub fn new_password_placeholder(&self) -> &'static str {
        match self {
            Self::English => "New Master Password (min 8 chars)",
            Self::Turkish => "Yeni Ana Şifre (en az 8 karakter)",
        }
    }

    pub fn recover_vault_btn(&self) -> &'static str {
        match self {
            Self::English => "Recover Vault & Update Password",
            Self::Turkish => "Kasayı Kurtar ve Şifreyi Güncelle",
        }
    }

    pub fn settings_security_section(&self) -> &'static str {
        match self {
            Self::English => "Security & Vault",
            Self::Turkish => "Güvenlik ve Kasa",
        }
    }

    pub fn settings_change_pw_title(&self) -> &'static str {
        match self {
            Self::English => "Change Master Vault Password",
            Self::Turkish => "Ana Kasa Şifresini Değiştir",
        }
    }

    pub fn settings_change_pw_desc(&self) -> &'static str {
        match self {
            Self::English => "Instant 0.01s update via envelope encryption (DEK/KEK) without re-encrypting files.",
            Self::Turkish => "Zarf şifreleme (DEK/KEK) sayesinde dosyalar yeniden şifrelenmeden 0.01 saniyede değiştirilir.",
        }
    }

    pub fn settings_current_pw_placeholder(&self) -> &'static str {
        match self {
            Self::English => "Current Master Password",
            Self::Turkish => "Mevcut Ana Şifre",
        }
    }

    pub fn settings_new_pw_placeholder(&self) -> &'static str {
        match self {
            Self::English => "New Master Password (min 8 chars)",
            Self::Turkish => "Yeni Ana Şifre (en az 8 karakter)",
        }
    }

    pub fn settings_change_pw_btn(&self) -> &'static str {
        match self {
            Self::English => "Change Password",
            Self::Turkish => "Şifreyi Değiştir",
        }
    }

    pub fn change_pw_header_btn(&self) -> &'static str {
        match self {
            Self::English => "Change Password",
            Self::Turkish => "Şifre Değiştir",
        }
    }

    pub fn confirm_new_password_placeholder(&self) -> &'static str {
        match self {
            Self::English => "Confirm New Password",
            Self::Turkish => "Yeni Şifreyi Doğrulayın",
        }
    }

    pub fn settings_pw_changed_msg(&self) -> &'static str {
        match self {
            Self::English => "Master password updated successfully!",
            Self::Turkish => "Ana şifre başarıyla güncellendi!",
        }
    }

    pub fn translate_error(&self, err: &str) -> String {
        let clean = err.trim();
        match self {
            Self::Turkish => {
                if clean.contains("Invalid master password") {
                    "Geçersiz ana şifre. Lütfen şifrenizi kontrol edin.".to_string()
                } else if clean.contains("Invalid recovery key") {
                    "Geçersiz kurtarma anahtarı.".to_string()
                } else if clean.contains("Invalid current master password") {
                    "Mevcut ana şifre hatalı.".to_string()
                } else if clean.contains("Daemon offline") || clean.contains("socket") {
                    "Arka plan servisi çevrimdışı. Bağlanılamadı.".to_string()
                } else if clean.contains("Vault has not been initialized") {
                    "Kasa henüz başlatılmamış.".to_string()
                } else {
                    clean.to_string()
                }
            }
            Self::English => {
                if clean.contains("Invalid master password") {
                    "Invalid master password. Please check your password.".to_string()
                } else if clean.contains("Invalid recovery key") {
                    "Invalid recovery key.".to_string()
                } else if clean.contains("Invalid current master password") {
                    "Current master password is incorrect.".to_string()
                } else {
                    clean.to_string()
                }
            }
        }
    }
}
