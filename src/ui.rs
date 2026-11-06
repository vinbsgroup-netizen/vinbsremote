use std::{
    collections::HashMap,
    iter::FromIterator,
    sync::{Arc, Mutex},
};

use sciter::Value;

use hbb_common::{
    allow_err,
    config::{LocalConfig, PeerConfig},
    log,
};

#[cfg(not(feature = "flutter"))]
use crate::ui_session_interface::Session;
use crate::{common::get_app_name, ipc, ui_interface::*};

mod cm;
#[cfg(feature = "inline")]
pub mod inline;
pub mod remote;

#[allow(dead_code)]
type Status = (i32, bool, i64, String);

lazy_static::lazy_static! {
    // stupid workaround for https://sciter.com/forums/topic/crash-on-latest-tis-mac-sdk-sometimes/
    static ref STUPID_VALUES: Mutex<Vec<Arc<Vec<Value>>>> = Default::default();
}

#[cfg(not(feature = "flutter"))]
lazy_static::lazy_static! {
    pub static ref CUR_SESSION: Arc<Mutex<Option<Session<remote::SciterHandler>>>> = Default::default();
}

struct UIHostHandler;

pub fn start(args: &mut [String]) {
    #[cfg(target_os = "macos")]
    crate::platform::delegate::show_dock();
    #[cfg(all(target_os = "linux", feature = "inline"))]
    {
        let app_dir = std::env::var("APPDIR").unwrap_or("".to_string());
        let mut so_path = "/usr/share/rustdesk/libsciter-gtk.so".to_owned();
        for (prefix, dir) in [
            ("", "/usr"),
            ("", "/app"),
            (&app_dir, "/usr"),
            (&app_dir, "/app"),
        ]
        .iter()
        {
            let path = format!("{prefix}{dir}/share/rustdesk/libsciter-gtk.so");
            if std::path::Path::new(&path).exists() {
                so_path = path;
                break;
            }
        }
        sciter::set_library(&so_path).ok();
    }
    #[cfg(windows)]
    // Check if there is a sciter.dll nearby.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let sciter_dll_path = parent.join("sciter.dll");
            if sciter_dll_path.exists() {
                // Try to set the sciter dll.
                let p = sciter_dll_path.to_string_lossy().to_string();
                log::debug!("Found dll:{}, \n {:?}", p, sciter::set_library(&p));
            }
        }
    }
    // https://github.com/c-smile/sciter-sdk/blob/master/include/sciter-x-types.h
    // https://github.com/rustdesk/rustdesk/issues/132#issuecomment-886069737
    #[cfg(windows)]
    allow_err!(sciter::set_options(sciter::RuntimeOptions::GfxLayer(
        sciter::GFX_LAYER::WARP
    )));
    use sciter::SCRIPT_RUNTIME_FEATURES::*;
    allow_err!(sciter::set_options(sciter::RuntimeOptions::ScriptFeatures(
        ALLOW_FILE_IO as u8 | ALLOW_SOCKET_IO as u8 | ALLOW_EVAL as u8 | ALLOW_SYSINFO as u8
    )));
    let mut frame = sciter::WindowBuilder::main_window().create();
    #[cfg(windows)]
    allow_err!(sciter::set_options(sciter::RuntimeOptions::UxTheming(true)));
    frame.set_title(crate::common::APP_DISPLAY_NAME);
    #[cfg(target_os = "macos")]
    crate::platform::delegate::make_menubar(frame.get_host(), args.is_empty());
    #[cfg(windows)]
    crate::platform::try_set_window_foreground(frame.get_hwnd() as _);
    let page;
    if args.len() > 1 && args[0] == "--play" {
        args[0] = "--connect".to_owned();
        let path: std::path::PathBuf = (&args[1]).into();
        let id = path
            .file_stem()
            .map(|p| p.to_str().unwrap_or(""))
            .unwrap_or("")
            .to_owned();
        args[1] = id;
    }
    if args.is_empty() {
        std::thread::spawn(move || check_zombie());
        crate::common::check_software_update();
        frame.event_handler(UI {});
        frame.sciter_handler(UIHostHandler {});
        page = "index.html";
        // Start pulse audio local server.
        #[cfg(target_os = "linux")]
        std::thread::spawn(crate::ipc::start_pa);
    } else if args[0] == "--install" {
        frame.event_handler(UI {});
        frame.sciter_handler(UIHostHandler {});
        page = "install.html";
    } else if args[0] == "--cm" {
        frame.register_behavior("connection-manager", move || {
            Box::new(cm::SciterConnectionManager::new())
        });
        page = "cm.html";
        *cm::HIDE_CM.lock().unwrap() = crate::ipc::get_config("hide_cm")
            .ok()
            .flatten()
            .unwrap_or_default()
            == "true";
    } else if (args[0] == "--connect"
        || args[0] == "--file-transfer"
        || args[0] == "--port-forward"
        || args[0] == "--rdp")
        && args.len() > 1
    {
        #[cfg(windows)]
        {
            let hw = frame.get_host().get_hwnd();
            crate::platform::windows::enable_lowlevel_keyboard(hw as _);
        }
        let mut iter = args.iter();
        let Some(cmd) = iter.next() else {
            log::error!("Failed to get cmd arg");
            return;
        };
        let cmd = cmd.to_owned();
        let Some(id) = iter.next() else {
            log::error!("Failed to get id arg");
            return;
        };
        let id = id.to_owned();
        let pass = iter.next().unwrap_or(&"".to_owned()).clone();
        let args: Vec<String> = iter.map(|x| x.clone()).collect();
        frame.set_title(&id);
        frame.register_behavior("native-remote", move || {
            let handler =
                remote::SciterSession::new(cmd.clone(), id.clone(), pass.clone(), args.clone());
            #[cfg(not(feature = "flutter"))]
            {
                *CUR_SESSION.lock().unwrap() = Some(handler.inner());
            }
            Box::new(handler)
        });
        page = "remote.html";
    } else {
        log::error!("Wrong command: {:?}", args);
        return;
    }
    #[cfg(feature = "inline")]
    {
        let html = if page == "index.html" {
            inline::get_index()
        } else if page == "cm.html" {
            inline::get_cm()
        } else if page == "install.html" {
            inline::get_install()
        } else {
            inline::get_remote()
        };
        frame.load_html(html.as_bytes(), Some(page));
    }
    #[cfg(not(feature = "inline"))]
    frame.load_file(&format!(
        "file://{}/src/ui/{}",
        std::env::current_dir()
            .map(|c| c.display().to_string())
            .unwrap_or("".to_owned()),
        page
    ));
    let hide_cm = *cm::HIDE_CM.lock().unwrap();
    if !args.is_empty() && args[0] == "--cm" && hide_cm {
        // run_app calls expand(show) + run_loop, we use collapse(hide) + run_loop instead to create a hidden window
        frame.collapse(true);
        frame.run_loop();
        return;
    }
    frame.run_app();
}

struct UI {}

impl UI {
    fn recent_sessions_updated(&self) -> bool {
        recent_sessions_updated()
    }

    fn get_id(&self) -> String {
        ipc::get_id()
    }

    fn temporary_password(&mut self) -> String {
        temporary_password()
    }

    fn update_temporary_password(&self) {
        update_temporary_password()
    }

    fn set_permanent_password(&self, password: String) {
        let _ = set_permanent_password_with_result(password);
    }

    fn is_local_permanent_password_set(&self) -> bool {
        is_local_permanent_password_set()
    }

    fn is_permanent_password_set(&self) -> bool {
        is_permanent_password_set()
    }

    fn get_remote_id(&mut self) -> String {
        LocalConfig::get_remote_id()
    }

    fn set_remote_id(&mut self, id: String) {
        LocalConfig::set_remote_id(&id);
    }

    fn goto_install(&mut self) {
        goto_install();
    }

    fn install_me(&mut self, _options: String, _path: String) {
        install_me(_options, _path, false, false);
    }

    fn update_me(&self, _path: String) {
        update_me(_path);
    }

    fn run_without_install(&self) {
        run_without_install();
    }

    fn show_run_without_install(&self) -> bool {
        show_run_without_install()
    }

    fn get_license(&self) -> String {
        get_license()
    }

    fn get_option(&self, key: String) -> String {
        get_option(key)
    }

    fn get_local_option(&self, key: String) -> String {
        get_local_option(key)
    }

    fn set_local_option(&self, key: String, value: String) {
        set_local_option(key, value);
    }

    fn peer_has_password(&self, id: String) -> bool {
        peer_has_password(id)
    }

    fn forget_password(&self, id: String) {
        forget_password(id)
    }

    fn get_peer_option(&self, id: String, name: String) -> String {
        get_peer_option(id, name)
    }

    fn set_peer_option(&self, id: String, name: String, value: String) {
        set_peer_option(id, name, value)
    }

    fn using_public_server(&self) -> bool {
        crate::using_public_server()
    }

    fn is_incoming_only(&self) -> bool {
        hbb_common::config::is_incoming_only()
    }

    pub fn is_outgoing_only(&self) -> bool {
        hbb_common::config::is_outgoing_only()
    }

    pub fn is_custom_client(&self) -> bool {
        crate::common::is_custom_client()
    }

    pub fn is_disable_settings(&self) -> bool {
        hbb_common::config::is_disable_settings()
    }

    pub fn is_disable_account(&self) -> bool {
        hbb_common::config::is_disable_account()
    }

    pub fn is_disable_installation(&self) -> bool {
        hbb_common::config::is_disable_installation()
    }

    pub fn is_disable_ab(&self) -> bool {
        hbb_common::config::is_disable_ab()
    }

    fn get_options(&self) -> Value {
        let hashmap: HashMap<String, String> =
            serde_json::from_str(&get_options()).unwrap_or_default();
        let mut m = Value::map();
        for (k, v) in hashmap {
            m.set_item(k, v);
        }
        m
    }

    fn test_if_valid_server(&self, host: String, test_with_proxy: bool) -> String {
        test_if_valid_server(host, test_with_proxy)
    }

    fn get_sound_inputs(&self) -> Value {
        Value::from_iter(get_sound_inputs())
    }

    fn set_options(&self, v: Value) {
        let mut m = HashMap::new();
        for (k, v) in v.items() {
            if let Some(k) = k.as_string() {
                if let Some(v) = v.as_string() {
                    if !v.is_empty() {
                        m.insert(k, v);
                    }
                }
            }
        }
        set_options(m);
    }

    fn set_option(&self, key: String, value: String) {
        set_option(key, value);
    }

    fn install_path(&mut self) -> String {
        install_path()
    }

    fn install_options(&self) -> String {
        install_options()
    }

    fn get_socks(&self) -> Value {
        Value::from_iter(get_socks())
    }

    fn set_socks(&self, proxy: String, username: String, password: String) {
        set_socks(proxy, username, password)
    }

    fn is_installed(&self) -> bool {
        is_installed()
    }

    fn get_supported_privacy_mode_impls(&self) -> String {
        serde_json::to_string(&crate::privacy_mode::get_supported_privacy_mode_impl())
            .unwrap_or_default()
    }

    fn is_root(&self) -> bool {
        is_root()
    }

    fn is_release(&self) -> bool {
        #[cfg(not(debug_assertions))]
        return true;
        #[cfg(debug_assertions)]
        return false;
    }

    fn is_share_rdp(&self) -> bool {
        is_share_rdp()
    }

    fn set_share_rdp(&self, _enable: bool) {
        set_share_rdp(_enable);
    }

    fn is_installed_lower_version(&self) -> bool {
        is_installed_lower_version()
    }

    fn closing(&mut self, x: i32, y: i32, w: i32, h: i32) {
        crate::server::input_service::fix_key_down_timeout_at_exit();
        LocalConfig::set_size(x, y, w, h);
    }

    fn get_size(&mut self) -> Value {
        let s = LocalConfig::get_size();
        let mut v = Vec::new();
        v.push(s.0);
        v.push(s.1);
        v.push(s.2);
        v.push(s.3);
        Value::from_iter(v)
    }

    fn get_mouse_time(&self) -> f64 {
        get_mouse_time()
    }

    fn check_mouse_time(&self) {
        check_mouse_time()
    }

    fn get_connect_status(&mut self) -> Value {
        let mut v = Value::array(0);
        let x = get_connect_status();
        v.push(x.status_num);
        v.push(x.key_confirmed);
        v.push(x.id);
        v
    }

    #[inline]
    fn get_peer_value(id: String, p: PeerConfig) -> Value {
        let values = vec![
            id,
            p.info.username.clone(),
            p.info.hostname.clone(),
            p.info.platform.clone(),
            p.options.get("alias").unwrap_or(&"".to_owned()).to_owned(),
        ];
        Value::from_iter(values)
    }

    fn get_peer(&self, id: String) -> Value {
        let c = get_peer(id.clone());
        Self::get_peer_value(id, c)
    }

    fn get_fav(&self) -> Value {
        Value::from_iter(get_fav())
    }

    fn store_fav(&self, fav: Value) {
        let mut tmp = vec![];
        fav.values().for_each(|v| {
            if let Some(v) = v.as_string() {
                if !v.is_empty() {
                    tmp.push(v);
                }
            }
        });
        store_fav(tmp);
    }

    fn get_recent_sessions(&mut self) -> Value {
        // to-do: limit number of recent sessions, and remove old peer file
        let peers: Vec<Value> = PeerConfig::peers(None)
            .drain(..)
            .map(|p| Self::get_peer_value(p.0, p.2))
            .collect();
        Value::from_iter(peers)
    }

    fn get_icon(&mut self) -> String {
        get_icon()
    }

    fn remove_peer(&mut self, id: String) {
        PeerConfig::remove(&id);
    }

    fn remove_discovered(&mut self, id: String) {
        remove_discovered(id);
    }

    fn send_wol(&mut self, id: String) {
        crate::lan::send_wol(id)
    }

    fn new_remote(&mut self, id: String, remote_type: String, force_relay: bool) {
        new_remote(id, remote_type, force_relay)
    }

    fn is_process_trusted(&mut self, _prompt: bool) -> bool {
        is_process_trusted(_prompt)
    }

    fn is_can_screen_recording(&mut self, _prompt: bool) -> bool {
        is_can_screen_recording(_prompt)
    }

    fn is_installed_daemon(&mut self, _prompt: bool) -> bool {
        is_installed_daemon(_prompt)
    }

    fn get_error(&mut self) -> String {
        get_error()
    }

    fn is_login_wayland(&mut self) -> bool {
        is_login_wayland()
    }

    fn current_is_wayland(&mut self) -> bool {
        current_is_wayland()
    }

    fn get_software_update_url(&self) -> String {
        crate::SOFTWARE_UPDATE_URL.lock().unwrap().clone()
    }

    fn get_new_version(&self) -> String {
        get_new_version()
    }

    fn get_version(&self) -> String {
        get_version()
    }

    fn get_fingerprint(&self) -> String {
        get_fingerprint()
    }

    fn get_app_name(&self) -> String {
        get_app_name()
    }

    fn get_app_display_name(&self) -> String {
        crate::common::APP_DISPLAY_NAME.to_owned()
    }

    fn get_logo(&self) -> String {
        let s = hbb_common::sodiumoxide::base64::encode(
            include_bytes!("../res/logo.png"),
            hbb_common::sodiumoxide::base64::Variant::Original,
        );
        format!("data:image/png;base64,{s}")
    }

    fn get_software_ext(&self) -> String {
        #[cfg(windows)]
        let p = "exe";
        #[cfg(target_os = "macos")]
        let p = "dmg";
        #[cfg(target_os = "linux")]
        let p = "deb";
        p.to_owned()
    }

    fn get_software_store_path(&self) -> String {
        let mut p = std::env::temp_dir();
        let name = crate::SOFTWARE_UPDATE_URL
            .lock()
            .unwrap()
            .split("/")
            .last()
            .map(|x| x.to_owned())
            .unwrap_or(crate::get_app_name());
        p.push(name);
        format!("{}.{}", p.to_string_lossy(), self.get_software_ext())
    }

    fn create_shortcut(&self, _id: String) {
        #[cfg(windows)]
        create_shortcut(_id)
    }

    fn discover(&self) {
        std::thread::spawn(move || {
            allow_err!(crate::lan::discover());
        });
    }

    fn get_lan_peers(&self) -> String {
        // let peers = get_lan_peers()
        //     .into_iter()
        //     .map(|mut peer| {
        //         (
        //             peer.remove("id").unwrap_or_default(),
        //             peer.remove("username").unwrap_or_default(),
        //             peer.remove("hostname").unwrap_or_default(),
        //             peer.remove("platform").unwrap_or_default(),
        //         )
        //     })
        //     .collect::<Vec<(String, String, String, String)>>();
        serde_json::to_string(&get_lan_peers()).unwrap_or_default()
    }

    fn get_uuid(&self) -> String {
        get_uuid()
    }

    fn open_url(&self, url: String) {
        #[cfg(windows)]
        let p = "explorer";
        #[cfg(target_os = "macos")]
        let p = "open";
        #[cfg(target_os = "linux")]
        let p = if std::path::Path::new("/usr/bin/firefox").exists() {
            "firefox"
        } else {
            "xdg-open"
        };
        allow_err!(std::process::Command::new(p).arg(url).spawn());
    }

    fn change_id(&self, id: String) {
        reset_async_job_status();
        let old_id = self.get_id();
        change_id_shared(id, old_id);
    }

    fn http_request(&self, url: String, method: String, body: Option<String>, header: String) {
        http_request(url, method, body, header)
    }

    fn post_request(&self, url: String, body: String, header: String) {
        post_request(url, body, header)
    }

    fn is_ok_change_id(&self) -> bool {
        hbb_common::machine_uid::get().is_ok()
    }

    fn get_async_job_status(&self) -> String {
        get_async_job_status()
    }

    fn get_http_status(&self, url: String) -> Option<String> {
        get_async_http_status(url)
    }

    fn t(&self, name: String) -> String {
        crate::client::translate(name)
    }

    fn is_xfce(&self) -> bool {
        crate::platform::is_xfce()
    }

    fn get_api_server(&self) -> String {
        get_api_server()
    }

    fn has_hwcodec(&self) -> bool {
        has_hwcodec()
    }

    fn has_vram(&self) -> bool {
        has_vram()
    }

    fn get_langs(&self) -> String {
        get_langs()
    }

    fn video_save_directory(&self, root: bool) -> String {
        video_save_directory(root)
    }

    fn handle_relay_id(&self, id: String) -> String {
        handle_relay_id(&id).to_owned()
    }

    fn get_login_device_info(&self) -> String {
        get_login_device_info_json()
    }

    fn support_remove_wallpaper(&self) -> bool {
        support_remove_wallpaper()
    }

    fn has_valid_2fa(&self) -> bool {
        has_valid_2fa()
    }

    fn generate2fa(&self) -> String {
        generate2fa()
    }

    pub fn verify2fa(&self, code: String) -> bool {
        verify2fa(code)
    }

    fn verify_login(&self, raw: String, id: String) -> bool {
        crate::verify_login(&raw, &id)
    }

    fn generate_2fa_img_src(&self, data: String) -> String {
        let v = qrcode_generator::to_png_to_vec(data, qrcode_generator::QrCodeEcc::Low, 128)
            .unwrap_or_default();
        let s = hbb_common::sodiumoxide::base64::encode(
            v,
            hbb_common::sodiumoxide::base64::Variant::Original,
        );
        format!("data:image/png;base64,{s}")
    }

    pub fn check_hwcodec(&self) {
        check_hwcodec()
    }

    fn is_option_fixed(&self, key: String) -> bool {
        crate::ui_interface::is_option_fixed(&key)
    }

    fn get_builtin_option(&self, key: String) -> String {
        crate::ui_interface::get_builtin_option(&key)
    }

    fn is_remote_modify_enabled_by_control_permissions(&self) -> String {
        match crate::ui_interface::is_remote_modify_enabled_by_control_permissions() {
            Some(true) => "true",
            Some(false) => "false",
            None => "",
        }
        .to_string()
    }
}

impl sciter::EventHandler for UI {
    sciter::dispatch_script_call! {
        fn t(String);
        fn get_api_server();
        fn is_xfce();
        fn using_public_server();
        fn is_custom_client();
        fn is_outgoing_only();
        fn is_incoming_only();
        fn is_disable_settings();
        fn is_disable_account();
        fn is_disable_installation();
        fn is_disable_ab();
        fn get_id();
        fn temporary_password();
        fn update_temporary_password();
        fn set_permanent_password(String);
        fn is_local_permanent_password_set();
        fn is_permanent_password_set();
        fn get_remote_id();
        fn set_remote_id(String);
        fn closing(i32, i32, i32, i32);
        fn get_size();
        fn new_remote(String, String, bool);
        fn send_wol(String);
        fn remove_peer(String);
        fn remove_discovered(String);
        fn get_connect_status();
        fn get_mouse_time();
        fn check_mouse_time();
        fn get_recent_sessions();
        fn get_peer(String);
        fn get_fav();
        fn store_fav(Value);
        fn recent_sessions_updated();
        fn get_icon();
        fn install_me(String, String);
        fn is_installed();
        fn get_supported_privacy_mode_impls();
        fn is_root();
        fn is_release();
        fn set_socks(String, String, String);
        fn get_socks();
        fn is_share_rdp();
        fn set_share_rdp(bool);
        fn is_installed_lower_version();
        fn install_path();
        fn install_options();
        fn goto_install();
        fn is_process_trusted(bool);
        fn is_can_screen_recording(bool);
        fn is_installed_daemon(bool);
        fn get_error();
        fn is_login_wayland();
        fn current_is_wayland();
        fn get_options();
        fn get_option(String);
        fn get_local_option(String);
        fn set_local_option(String, String);
        fn get_peer_option(String, String);
        fn peer_has_password(String);
        fn forget_password(String);
        fn set_peer_option(String, String, String);
        fn get_license();
        fn test_if_valid_server(String, bool);
        fn get_sound_inputs();
        fn set_options(Value);
        fn set_option(String, String);
        fn get_software_update_url();
        fn get_new_version();
        fn get_version();
        fn get_fingerprint();
        fn update_me(String);
        fn show_run_without_install();
        fn run_without_install();
        fn get_app_name();
        fn get_app_display_name();
        fn get_logo();
        fn get_software_store_path();
        fn get_software_ext();
        fn open_url(String);
        fn change_id(String);
        fn get_async_job_status();
        fn post_request(String, String, String);
        fn is_ok_change_id();
        fn create_shortcut(String);
        fn discover();
        fn get_lan_peers();
        fn get_uuid();
        fn has_hwcodec();
        fn has_vram();
        fn get_langs();
        fn video_save_directory(bool);
        fn handle_relay_id(String);
        fn get_login_device_info();
        fn support_remove_wallpaper();
        fn has_valid_2fa();
        fn generate2fa();
        fn generate_2fa_img_src(String);
        fn verify2fa(String);
        fn check_hwcodec();
        fn verify_login(String, String);
        fn is_option_fixed(String);
        fn get_builtin_option(String);
        fn is_remote_modify_enabled_by_control_permissions();
    }
}

impl sciter::host::HostHandler for UIHostHandler {
    fn on_graphics_critical_failure(&mut self) {
        log::error!("Critical rendering error: e.g. DirectX gfx driver error. Most probably bad gfx drivers.");
    }
}

#[cfg(not(target_os = "linux"))]
fn get_sound_inputs() -> Vec<String> {
    let mut out = Vec::new();
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    if let Ok(devices) = host.devices() {
        for device in devices {
            if device.default_input_config().is_err() {
                continue;
            }
            if let Ok(name) = device.name() {
                out.push(name);
            }
        }
    }
    out
}

#[cfg(target_os = "linux")]
fn get_sound_inputs() -> Vec<String> {
    crate::platform::linux::get_pa_sources()
        .drain(..)
        .map(|x| x.1)
        .collect()
}

// sacrifice some memory
pub fn value_crash_workaround(values: &[Value]) -> Arc<Vec<Value>> {
    let persist = Arc::new(values.to_vec());
    STUPID_VALUES.lock().unwrap().push(persist.clone());
    persist
}

pub fn get_icon() -> String {
    // 128x128
    #[cfg(target_os = "macos")]
    // 128x128 on 160x160 canvas, then shrink to 128, mac looks better with padding
    {
        "data:image/png;base64,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".into()
    }
    #[cfg(not(target_os = "macos"))] // 128x128 no padding
    {
        "data:image/png;base64,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".into()
    }
}
