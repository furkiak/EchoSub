#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod langs;
mod models;
mod pipeline;
mod separate;
mod settings;
mod sidecar;

use pipeline::{Caption, Pipeline};
use serde::Serialize;
use settings::Settings;
use sidecar::{Paths, Sidecars, LLAMA_PORT, WHISPER_PORT};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, WebviewUrl, WebviewWindowBuilder};

#[derive(Serialize, Clone, Default)]
struct Status {
    stage: String, // "loading" | "ready" | "warn" | "error"
    msg: String,
}

struct App {
    settings: Arc<Mutex<Settings>>,
    settings_path: PathBuf,
    paths: Paths,
    pipeline: Pipeline,
    sidecars: Arc<Mutex<Sidecars>>,
    status: Mutex<Status>,
    boot_lock: Mutex<()>,
    /// Model indirmesi sürüyor mu / iptal istendi mi.
    dl_active: AtomicBool,
    dl_cancel: AtomicBool,
    /// İndirme penceresini hangi eksik model kümesi için en son açtık (aynı küme için tekrar açma).
    asked: Mutex<Vec<String>>,
}

/// Aynı anda tek kopya: görev çubuğunda görünmediği için kullanıcı kolayca iki kez başlatabilir.
const INSTANCE_PORT: u16 = 18079;
/// app.log bu boyutu geçince (başlangıçta) app.old.log olur.
const LOG_MAX_BYTES: u64 = 5 * 1024 * 1024;

fn rotate_log(paths: &Paths) {
    let log = paths.log_dir().join("app.log");
    if std::fs::metadata(&log).map(|m| m.len() > LOG_MAX_BYTES).unwrap_or(false) {
        let _ = std::fs::rename(&log, paths.log_dir().join("app.old.log"));
    }
}

/// Windows: uygulama nasıl kapanırsa kapansın (Görev Yöneticisi, çökme) alt süreçler de kapansın;
/// yoksa sunucular VRAM'i ve portları tutmaya devam eder.
#[cfg(windows)]
fn kill_children_on_exit() {
    use std::mem::{size_of, zeroed};
    use std::ptr::null;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    // Tutamaç bilerek kapatılmaz: süreç ölünce sistem kapatır, iş nesnesi alt süreçleri sonlandırır.
    unsafe {
        let job = CreateJobObjectW(null(), null());
        if job.is_null() {
            return;
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        AssignProcessToJobObject(job, GetCurrentProcess());
    }
}

fn log_line(paths: &Paths, msg: &str) {
    let dir = paths.log_dir();
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("app.log")) {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() % 100000)
            .unwrap_or(0);
        let _ = writeln!(f, "[{t}] {msg}");
    }
}

fn set_status(app: &AppHandle, stage: &str, msg: &str) {
    let st = app.state::<App>();
    let s = Status { stage: stage.into(), msg: msg.into() };
    *st.status.lock().unwrap() = s.clone();
    log_line(&st.paths, &format!("durum {stage}: {msg}"));
    let _ = app.emit("status", s);
}

/// Eksik model varsa kullanıcıya sor (aynı eksik kümesi için bir kez).
fn ask_for_missing_models(app: &AppHandle) {
    let st = app.state::<App>();
    let s = st.settings.lock().unwrap().clone();
    let miss = models::missing(&st.paths, &s);
    if miss.is_empty() || st.dl_active.load(Ordering::Relaxed) {
        return;
    }
    let key: Vec<String> = miss.iter().map(|m| m.file.to_string()).collect();
    let first_time = {
        let mut asked = st.asked.lock().unwrap();
        let fresh = *asked != key;
        *asked = key;
        fresh
    };
    if first_time {
        open_models_window(app);
    }
}

/// Sunucuları (yeniden) başlatır; model yüklenirken durum olayları gönderir.
fn boot(app: AppHandle) {
    let st = app.state::<App>();
    let _g = st.boot_lock.lock().unwrap();
    st.pipeline.whisper_ready.store(false, Ordering::Relaxed);
    st.pipeline.llama_ready.store(false, Ordering::Relaxed);
    st.sidecars.lock().unwrap().stop();
    let s = st.settings.lock().unwrap().clone();

    ask_for_missing_models(&app);
    let (wm, _) = s.model_files();
    if !models::is_present(&st.paths.model(wm)) {
        set_status(&app, "error", "Modeller henüz indirilmedi — ayarlardan “Modelleri indir” düğmesini kullan");
        return;
    }

    set_status(&app, "loading", "Konuşma modeli yükleniyor…");
    st.pipeline.vad_ready.store(st.paths.model(sidecar::VAD_FILE).exists(), Ordering::Relaxed);
    let started = {
        // Kapanış, kilidi almadan önce `stop` der: kapanırken yeni sunucu başlatılmaz.
        let mut sc = st.sidecars.lock().unwrap();
        if st.pipeline.stop.load(Ordering::Relaxed) {
            return;
        }
        sc.start_whisper(&st.paths, &s)
    };
    match started {
        Ok(()) => {
            let url = format!("http://127.0.0.1:{WHISPER_PORT}/");
            let alive = || st.sidecars.lock().unwrap().whisper_alive();
            match sidecar::wait_http(&url, Duration::from_secs(120), alive) {
                sidecar::Wait::Ready => {
                    pipeline::warmup_whisper();
                    if !alive() {
                        set_status(&app, "error", WHISPER_CRASHED);
                        return;
                    }
                    st.pipeline.whisper_ready.store(true, Ordering::Relaxed);
                }
                sidecar::Wait::Died => {
                    set_status(&app, "error", WHISPER_CRASHED);
                    return;
                }
                sidecar::Wait::Timeout => {
                    set_status(&app, "error", "Konuşma modeli yüklenemedi (zaman aşımı)");
                    return;
                }
            }
        }
        Err(e) => {
            set_status(&app, "error", &e);
            return;
        }
    }

    set_status(&app, "loading", "Çeviri modeli yükleniyor…");
    let started = {
        let mut sc = st.sidecars.lock().unwrap();
        if st.pipeline.stop.load(Ordering::Relaxed) {
            return;
        }
        sc.start_llama(&st.paths, &s)
    };
    match started {
        Ok(()) => {
            let url = format!("http://127.0.0.1:{LLAMA_PORT}/health");
            let alive = || st.sidecars.lock().unwrap().llama_alive();
            match sidecar::wait_http(&url, Duration::from_secs(240), alive) {
                sidecar::Wait::Ready => {
                    pipeline::warmup_llama();
                    if !alive() {
                        set_status(&app, "warn", LLAMA_CRASHED);
                        return;
                    }
                    st.pipeline.llama_ready.store(true, Ordering::Relaxed);
                    set_status(&app, "ready", "Hazır");
                }
                sidecar::Wait::Died => {
                    set_status(&app, "warn", LLAMA_CRASHED);
                }
                sidecar::Wait::Timeout => {
                    set_status(&app, "warn", "Çeviri modeli yüklenemedi; yalnızca orijinal altyazı");
                }
            }
        }
        Err(e) => set_status(&app, "warn", &format!("Çeviri kapalı: {e}")),
    }
}

const WHISPER_CRASHED: &str = "Konuşma sunucusu çöktü — ayrıntı: runtime/logs/whisper.log";
const LLAMA_CRASHED: &str = "Çeviri sunucusu çöktü (runtime/logs/llama.log); yalnızca orijinal altyazı";

/// Sunucu çalışırken kapanırsa (sürücü sıfırlanması gibi) altyazı sessizce kesilmesin.
fn spawn_server_watchdog(app: AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(2));
        let st = app.state::<App>();
        let p = &st.pipeline;
        if p.stop.load(Ordering::Relaxed) {
            return;
        }
        // boot önce "hazır" bayrağını indirip sonra durdurduğu için kilit altında yanlış alarm olmaz.
        let (whisper_down, llama_down) = {
            let mut sc = st.sidecars.lock().unwrap();
            (
                p.whisper_ready.load(Ordering::Relaxed) && !sc.whisper_alive(),
                p.llama_ready.load(Ordering::Relaxed) && !sc.llama_alive(),
            )
        };
        if whisper_down {
            p.whisper_ready.store(false, Ordering::Relaxed);
            set_status(&app, "error", WHISPER_CRASHED);
        }
        if llama_down {
            p.llama_ready.store(false, Ordering::Relaxed);
            set_status(&app, "warn", LLAMA_CRASHED);
        }
    });
}

/// Altyazı penceresi için imleç izleyici (~25 Hz).
///
/// - `hover` olayı: imleç pencerenin üstündeyken arayüz şeridini/çerçevesini göstermek için.
///   (Tıklama geçirme açıkken WebView fare olayı almaz; bu yüzden imleç konumu buradan izlenir.)
/// - Tıklama geçirme: imleç yalnızca üst şerit ya da kenarlardayken pencere tıklama
///   alır; metin alanı üzerindeyken tıklamalar alttaki pencereye gider.
fn spawn_hover_watcher(app: AppHandle) {
    use std::time::Instant;
    // Arayüzdeki ölçülerle aynı olmalı (CSS px): üst şerit yüksekliği ve kenar kalınlığı.
    const BAR_PX: f64 = 38.0;
    const EDGE_PX: f64 = 9.0;
    std::thread::spawn(move || {
        let mut ignoring: Option<bool> = None;
        let mut last_hover = false;
        let (mut fails, mut tracking) = (0u32, true);
        // Pencere yeni taşındı/boyutlandıysa (sürükleme sürüyor) kısa süre etkileşimli kal;
        // yoksa şeritten ayrılınca tıklamalar hemen alttaki pencereye gitmeli.
        let mut motion_until = Instant::now();
        let mut last_geom = (0i32, 0i32, 0u32, 0u32);
        loop {
            std::thread::sleep(Duration::from_millis(40));
            let Some(win) = app.get_webview_window("main") else { continue };
            let click_through = app.state::<App>().settings.lock().unwrap().click_through;
            let (Ok(cur), Ok(pos), Ok(size), Ok(scale)) =
                (app.cursor_position(), win.outer_position(), win.outer_size(), win.scale_factor())
            else {
                // İmleç konumu okunamıyor (ör. Wayland'da başka pencerenin üstündeyken).
                // Yaklaşık 1 sn sonra tıklama geçirmeden vazgeç: pencere tamamen etkileşimli
                // kalır, arayüz CSS :hover ile çalışır ve her yerden sürüklenir.
                fails += 1;
                if fails == 25 {
                    let _ = win.set_ignore_cursor_events(false);
                    ignoring = Some(false);
                    tracking = false;
                    let _ = app.emit_to("main", "tracking", false);
                    let p = app.state::<App>().paths.clone();
                    log_line(&p, "imleç konumu okunamıyor: tıklama geçirme devre dışı");
                }
                continue;
            };
            fails = 0;
            if !tracking {
                tracking = true;
                ignoring = None;
                let _ = app.emit_to("main", "tracking", true);
            }
            let (rx, ry) = (cur.x - pos.x as f64, cur.y - pos.y as f64);
            let (w, h) = (size.width as f64, size.height as f64);
            let inside = rx >= 0.0 && ry >= 0.0 && rx < w && ry < h;
            // Sürüklerken pencere imleçten kısa süre geri kalabilir; bölgeyi genişlet.
            let m = 48.0 * scale;
            let near = rx >= -m && ry >= -m && rx < w + m && ry < h + m;
            let (bar, edge) = (BAR_PX * scale, EDGE_PX * scale);
            let in_zone = inside && (ry < bar || rx < edge || rx > w - edge || ry > h - edge);
            let now = Instant::now();
            let geom = (pos.x, pos.y, size.width, size.height);
            if geom != last_geom {
                last_geom = geom;
                motion_until = now + Duration::from_millis(300);
            }
            let held = near && now < motion_until;
            let interactive = !click_through || in_zone || held;
            let hover = inside || held;

            if ignoring != Some(!interactive) {
                let r = win.set_ignore_cursor_events(!interactive);
                ignoring = Some(!interactive);
                if std::env::var("ECHOSUB_DEBUG_HOVER").is_ok() {
                    let p = app.state::<App>().paths.clone();
                    log_line(
                        &p,
                        &format!(
                            "izleyici: ignore={} (ct={click_through} zone={in_zone} held={held}) rx={rx:.0} ry={ry:.0} sonuç={r:?}",
                            !interactive
                        ),
                    );
                }
            }
            if hover != last_hover {
                let _ = app.emit_to("main", "hover", hover);
                last_hover = hover;
            }
        }
    });
}

#[tauri::command]
fn get_settings(state: State<App>) -> Settings {
    state.settings.lock().unwrap().clone()
}

#[tauri::command]
fn get_status(state: State<App>) -> Status {
    state.status.lock().unwrap().clone()
}

#[tauri::command]
fn languages() -> Vec<(String, String)> {
    langs::LANGS.iter().map(|(c, _)| (c.to_string(), langs::display_name(c))).collect()
}

#[tauri::command]
fn set_settings(app: AppHandle, state: State<App>, new: Settings) {
    let (restart, separation_changed) = {
        let mut cur = state.settings.lock().unwrap();
        let r = (cur.needs_restart(&new), cur.use_separation != new.use_separation);
        *cur = new.clone();
        r
    };
    new.save(&state.settings_path);
    let _ = app.emit("settings", new);
    let h = app.clone();
    if restart {
        std::thread::spawn(move || boot(h));
    } else if separation_changed {
        // Ayrı iş parçacığından: senkron komuttan pencere açmak WebView'i kilitler.
        std::thread::spawn(move || ask_for_missing_models(&h));
    }
}

// async: Windows'ta senkron komuttan pencere oluşturmak WebView'i kilitler.
#[tauri::command]
async fn open_settings(app: AppHandle) {
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let _ = WebviewWindowBuilder::new(&app, "settings", WebviewUrl::App("settings.html".into()))
        .title("EchoSub – Ayarlar")
        .inner_size(500.0, 760.0)
        .always_on_top(true)
        .build();
}

fn open_models_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("models") {
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let _ = WebviewWindowBuilder::new(app, "models", WebviewUrl::App("models.html".into()))
        .title("EchoSub – Modeller")
        .inner_size(520.0, 560.0)
        .center()
        .always_on_top(true)
        .build();
}

#[tauri::command]
async fn open_models(app: AppHandle) {
    open_models_window(&app);
}

#[derive(Serialize)]
struct ModelInfo {
    file: String,
    label: String,
    mb: u32,
    present: bool,
}

/// Geçerli ayarlar için gereken modeller ve hangilerinin diskte olduğu.
#[tauri::command]
fn model_list(state: State<App>) -> Vec<ModelInfo> {
    let s = state.settings.lock().unwrap().clone();
    models::required(&s)
        .into_iter()
        .map(|m| ModelInfo {
            file: m.file.into(),
            label: m.label.into(),
            mb: m.mb,
            present: models::is_present(&state.paths.model(m.file)),
        })
        .collect()
}

#[derive(Serialize, Clone)]
struct DlEvent {
    file: String,
    label: String,
    index: usize,
    count: usize,
    done: u64,
    total: u64,
}

#[derive(Serialize, Clone)]
struct DlDone {
    ok: bool,
    error: Option<String>,
}

fn run_download(app: AppHandle) {
    let st = app.state::<App>();
    let s = st.settings.lock().unwrap().clone();
    let todo = models::missing(&st.paths, &s);
    let dir = st.paths.model_dir();
    let count = todo.len();
    let mut result: Result<(), String> = Ok(());
    for (i, m) in todo.iter().enumerate() {
        log_line(&st.paths, &format!("indiriliyor: {}", m.file));
        let r = models::download(m, &dir, &st.dl_cancel, &mut |done, total| {
            let _ = app.emit(
                "dl",
                DlEvent { file: m.file.into(), label: m.label.into(), index: i + 1, count, done, total },
            );
        });
        if let Err(e) = r {
            result = Err(format!("{}: {e}", m.file));
            break;
        }
    }
    match &result {
        Ok(()) => log_line(&st.paths, "model indirme tamamlandı"),
        Err(e) => log_line(&st.paths, &format!("model indirme durdu: {e}")),
    }
    st.dl_active.store(false, Ordering::Relaxed);
    let ok = result.is_ok();
    let _ = app.emit("dl_done", DlDone { ok, error: result.err() });
    if ok {
        let h = app.clone();
        std::thread::spawn(move || boot(h));
    }
}

#[tauri::command]
fn download_models(app: AppHandle, state: State<App>) {
    if state.dl_active.swap(true, Ordering::SeqCst) {
        return;
    }
    state.dl_cancel.store(false, Ordering::Relaxed);
    std::thread::spawn(move || run_download(app));
}

#[tauri::command]
fn cancel_download(state: State<App>) {
    state.dl_cancel.store(true, Ordering::Relaxed);
}

#[tauri::command]
fn quit(app: AppHandle) {
    app.exit(0);
}

/// Geliştirme aracı: `echosub --separate girdi.wav cikti.wav model.onnx`
/// (16 kHz mono 16-bit WAV). Süreler `cikti.wav.txt` dosyasına yazılır.
fn cli_separate(args: &[String]) {
    let (inp, out, model) = (&args[2], &args[3], &args[4]);
    let bytes = std::fs::read(inp).expect("girdi okunamadı");
    let data_at = bytes.windows(4).position(|w| w == b"data").expect("data bölümü yok") + 8;
    let pcm: Vec<f32> = bytes[data_at..]
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
        .collect();
    let mut report = String::new();
    let t0 = std::time::Instant::now();
    let mut sep = match separate::Separator::load(std::path::Path::new(model)) {
        Ok(s) => s,
        Err(e) => {
            let _ = std::fs::write(format!("{out}.txt"), format!("model yüklenemedi: {e}"));
            return;
        }
    };
    report += &format!("yükleme: {} ms\n", t0.elapsed().as_millis());
    for run in 1..=2 {
        let t = std::time::Instant::now();
        match sep.vocals(&pcm) {
            Ok(v) => {
                report += &format!(
                    "ayırma #{run}: {} ms ({:.1} sn ses)\n",
                    t.elapsed().as_millis(),
                    pcm.len() as f32 / 16000.0
                );
                if run == 2 {
                    let _ = std::fs::write(out, pipeline::encode_wav(&v));
                }
            }
            Err(e) => report += &format!("hata: {e}\n"),
        }
    }
    let _ = std::fs::write(format!("{out}.txt"), report);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--separate") && args.len() >= 5 {
        cli_separate(&args);
        return;
    }
    // Kapanana kadar tutulur.
    let _instance = match std::net::TcpListener::bind(("127.0.0.1", INSTANCE_PORT)) {
        Ok(l) => l,
        Err(_) => {
            log_line(&Paths::resolve(), "EchoSub zaten açık; ikinci kopya kapatıldı");
            return;
        }
    };
    #[cfg(windows)]
    kill_children_on_exit();
    let app = tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            get_settings, set_settings, get_status, languages, open_settings, quit,
            open_models, model_list, download_models, cancel_download
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            let paths = Paths::resolve();
            rotate_log(&paths);
            let cfg_dir = app.path().app_config_dir().unwrap_or_else(|_| PathBuf::from("."));
            let settings_path = cfg_dir.join("settings.json");
            let settings = Arc::new(Mutex::new(Settings::load(&settings_path)));

            app.manage(App {
                settings: Arc::clone(&settings),
                settings_path,
                paths: paths.clone(),
                pipeline: Pipeline::new(),
                sidecars: Arc::new(Mutex::new(Sidecars::default())),
                status: Mutex::new(Status { stage: "loading".into(), msg: "Başlatılıyor…".into() }),
                boot_lock: Mutex::new(()),
                dl_active: AtomicBool::new(false),
                dl_cancel: AtomicBool::new(false),
                asked: Mutex::new(Vec::new()),
            });
            log_line(&paths, &format!("başlatıldı, home={}", paths.home.display()));

            // Pencere: ekranın altında, ortada.
            if let Some(win) = app.get_webview_window("main") {
                if let (Ok(Some(m)), Ok(ws)) = (win.primary_monitor(), win.outer_size()) {
                    let scale = m.scale_factor();
                    let x = m.position().x + (m.size().width as i32 - ws.width as i32) / 2;
                    let y = m.position().y + m.size().height as i32 - ws.height as i32 - (90.0 * scale) as i32;
                    let _ = win.set_position(tauri::PhysicalPosition::new(x, y));
                }
                let _ = win.show();
            }

            // Ses yakalama + hat.
            let shared: audio::Shared = Arc::new(Mutex::new(Vec::new()));
            let st = app.state::<App>();
            let log_paths = paths.clone();
            let log: pipeline::LogFn = Arc::new(move |m| log_line(&log_paths, m));
            {
                let lg = Arc::clone(&log);
                audio::spawn(Arc::clone(&shared), Arc::clone(&st.pipeline.stop), move |m| lg(m));
            }
            let emit_handle = handle.clone();
            let emit_paths = paths.clone();
            // Duyulan her şey varsayılan olarak diske yazılmaz (görüşmeler de buradan geçer).
            let log_captions = std::env::var("ECHOSUB_LOG_CAPTIONS").is_ok();
            let emit: pipeline::EmitFn = Arc::new(move |c: &Caption| {
                if log_captions && !c.clear {
                    log_line(
                        &emit_paths,
                        &format!(
                            "{} #{} [{}] {} => {} ({} ms; {})",
                            if c.is_final { "FINAL" } else { "part " },
                            c.seg_id,
                            c.lang,
                            c.original,
                            c.translated.as_deref().unwrap_or("-"),
                            c.latency_ms,
                            c.timing
                        ),
                    );
                }
                let _ = emit_handle.emit("caption", c);
            });
            st.pipeline.start(shared, settings, emit, log, paths.model("UVR-MDX-NET-Voc_FT.onnx"));

            spawn_hover_watcher(handle.clone());
            spawn_server_watchdog(handle.clone());
            // Modelleri arka planda yükle.
            std::thread::spawn(move || boot(handle));
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("uygulama başlatılamadı");

    app.run(|handle, event| {
        if let RunEvent::Exit = event {
            let st = handle.state::<App>();
            st.pipeline.stop.store(true, Ordering::Relaxed);
            st.sidecars.lock().unwrap().stop();
        }
    });
}
