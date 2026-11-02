//! Konuşma bölütleme -> Whisper (dil algıla + yazıya dök) -> çeviri -> altyazı olayı.
//!
//! Bölütleyici: 20 ms'lik karelerle enerji ölçer. Konuşma başlayınca bir
//! "segment" açar, yaklaşık 0,7 sn'de bir kısmi (partial) sonuç ister, 400 ms
//! sessizlikte ya da 7 sn dolunca segmenti bitirir (en sessiz noktadan keser).
//!
//! Üç iş parçacığı boru hattı gibi çalışır: bölütleyici -> Whisper -> çeviri.
//! Böylece çeviri sürerken Whisper bir sonraki parçayı işleyebilir; bayatlamış
//! kısmi işler atlanır; segment sonunda yeni bir şey duyulmadıysa son kısmi
//! sonuç doğrudan "final" olarak kullanılır (Whisper'a tekrar gidilmez).

use crate::audio::{Shared, TARGET_RATE};
use crate::langs;
use crate::separate::Separator;
use crate::settings::Settings;
use crate::sidecar::{LLAMA_PORT, WHISPER_PORT};
use serde::Serialize;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

const FRAME: usize = 320; // 20 ms @ 16 kHz
const SPEECH_RMS: f32 = 0.003;
const END_SILENCE_MS: u32 = 400;
const MAX_SEG_SAMPLES: usize = 7 * TARGET_RATE as usize;
const MIN_VOICED_SAMPLES: usize = TARGET_RATE as usize * 4 / 10;
const PARTIAL_EVERY: Duration = Duration::from_millis(700);
const PARTIAL_MIN_SAMPLES: usize = TARGET_RATE as usize * 8 / 10;
const PRE_ROLL: usize = TARGET_RATE as usize * 3 / 10;
/// Son kısmi sonuçtan beri bu kadardan az yeni konuşma varsa final için
/// Whisper'a tekrar gidilmez.
/// Eşik küçük tutuldu: kısmi sonuç son kelimeyi yarım duymuş olabilir.
const REUSE_MAX_NEW_SAMPLES: usize = TARGET_RATE as usize / 10;
/// Yetişemeyen makinede (ekran kartı yok) en fazla bu kadar cümle bekler; eskisi atılır.
/// Yoksa altyazı dakikalarca geriden gelir.
const MAX_PENDING_FINALS: usize = 3;
/// Whisper bir parçayı bundan yavaş işliyorsa kısmi sonuçlar atlanır: sıra final cümlelere kalsın.
const WHISPER_SLOW_MS: u64 = 1500;
/// Sunucu isteği zaman aşımı. İşlemciyle tek parça onlarca saniye sürebilir; sunucu vazgeçilen
/// isteği işlemeye devam ettiği için kısa süre sonraki istekleri de aç bırakır.
const HTTP_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Serialize, Clone, Debug)]
pub struct Caption {
    pub seg_id: u64,
    #[serde(rename = "final")]
    pub is_final: bool,
    /// true ise bu segmentin altyazısını sil (boş/halüsinasyon sonucu).
    pub clear: bool,
    pub lang: String,
    pub original: String,
    pub translated: Option<String>,
    /// Konuşma bölümü bittikten (ya da kısmi istendikten) sonra geçen süre.
    pub latency_ms: u64,
    /// Günlük için: kuyruk / whisper / çeviri süreleri.
    #[serde(skip)]
    pub timing: String,
}

impl Caption {
    /// Bu segmentin altyazısını ekrandan kaldırır.
    fn cleared(seg_id: u64, is_final: bool) -> Self {
        Caption {
            seg_id,
            is_final,
            clear: true,
            lang: String::new(),
            original: String::new(),
            translated: None,
            latency_ms: 0,
            timing: String::new(),
        }
    }
}

pub type EmitFn = Arc<dyn Fn(&Caption) + Send + Sync>;
pub type LogFn = Arc<dyn Fn(&str) + Send + Sync>;

struct Job {
    seg_id: u64,
    pcm: Vec<f32>,
    is_final: bool,
    created: Instant,
    /// Final için: son kısmi sonuç bu final'i karşılıyorsa (yeni konuşma yok) o kısmi işin
    /// örnek sayısı; 0 ise Whisper'a yeniden gidilir.
    reuse_len: usize,
}

/// Son işlenen kısmi Whisper sonucu.
#[derive(Clone)]
struct Partial {
    seg_id: u64,
    /// Kaç örnek üzerinden üretildi.
    len: usize,
    text: String,
    language: String,
    no_speech: f32,
    lp: f32,
}

/// Final için kısmi sonuç kullanılabilir mi? Bölütleyicinin istediği son kısmi iş kuyrukta
/// düşmüş olabilir (final onun yerini alır, yavaş modda atlanır); o zaman elimizdeki sonuç
/// daha eskidir ve son kelimeleri içermez.
fn reusable_partial(last: &Option<Partial>, job: &Job) -> Option<Partial> {
    last.as_ref().filter(|p| job.reuse_len > 0 && p.seg_id == job.seg_id && p.len == job.reuse_len).cloned()
}

/// Whisper çıktısı, çeviri bekleyen iş.
struct TJob {
    seg_id: u64,
    is_final: bool,
    created: Instant,
    text: String,
    code: &'static str,
    q_ms: u64,
    w_ms: u64,
    lp: f32,
}

fn drop_oldest_finals<T>(q: &mut VecDeque<T>, is_final: impl Fn(&T) -> bool) {
    while q.iter().filter(|j| is_final(j)).count() > MAX_PENDING_FINALS {
        if let Some(i) = q.iter().position(|j| is_final(j)) {
            q.remove(i);
        }
    }
}

/// Sırası gelen iş atılmalı mı? En yeni iş (arkasında başka iş yok) hiç atılmaz: yavaş makinede
/// her şey sınırı aşar ve ekranda hiç altyazı kalmazdı. Sınır 0 ise atılmaz.
fn is_stale(age: Duration, limit_secs: f32, has_newer: bool) -> bool {
    has_newer && limit_secs > 0.0 && age.as_secs_f32() > limit_secs
}

#[derive(Default)]
struct TQueue {
    items: Mutex<VecDeque<TJob>>,
    cv: Condvar,
}

impl TQueue {
    fn push(&self, job: TJob) {
        let mut q = self.items.lock().unwrap();
        if job.is_final {
            q.retain(|j| j.seg_id != job.seg_id);
        } else {
            q.retain(|j| j.is_final);
        }
        q.push_back(job);
        drop_oldest_finals(&mut q, |j| j.is_final);
        self.cv.notify_one();
    }

    fn has_final(&self) -> bool {
        self.items.lock().unwrap().iter().any(|j| j.is_final)
    }

    fn is_empty(&self) -> bool {
        self.items.lock().unwrap().is_empty()
    }

    fn clear(&self) {
        self.items.lock().unwrap().clear();
    }

    fn pop(&self, stop: &AtomicBool) -> Option<TJob> {
        let mut q = self.items.lock().unwrap();
        loop {
            if let Some(j) = q.pop_front() {
                return Some(j);
            }
            if stop.load(Ordering::Relaxed) {
                return None;
            }
            q = self.cv.wait_timeout(q, Duration::from_millis(300)).unwrap().0;
        }
    }
}

#[derive(Default)]
struct Queue {
    items: Mutex<VecDeque<Job>>,
    cv: Condvar,
}

impl Queue {
    fn push(&self, job: Job) {
        let mut q = self.items.lock().unwrap();
        if job.is_final {
            q.retain(|j| j.seg_id != job.seg_id);
        } else {
            q.retain(|j| j.is_final); // en fazla bir bekleyen kısmi iş
        }
        q.push_back(job);
        drop_oldest_finals(&mut q, |j| j.is_final);
        self.cv.notify_one();
    }

    fn is_empty(&self) -> bool {
        self.items.lock().unwrap().is_empty()
    }

    fn clear(&self) {
        self.items.lock().unwrap().clear();
    }

    fn pop(&self, stop: &AtomicBool) -> Option<Job> {
        let mut q = self.items.lock().unwrap();
        loop {
            if let Some(j) = q.pop_front() {
                return Some(j);
            }
            if stop.load(Ordering::Relaxed) {
                return None;
            }
            q = self.cv.wait_timeout(q, Duration::from_millis(300)).unwrap().0;
        }
    }
}

pub struct Pipeline {
    pub stop: Arc<AtomicBool>,
    pub whisper_ready: Arc<AtomicBool>,
    pub llama_ready: Arc<AtomicBool>,
    /// Whisper sunucusu VAD modeliyle mi başlatıldı? Modelsiz sunucu `vad=true` isteklerini reddeder.
    pub vad_ready: Arc<AtomicBool>,
    /// Altyazı gizliyken ses atılır, sunuculara istek gitmez (ekran kartı oyuna kalır).
    /// Modeller bellekte kalır, böylece geri açmak anında olur.
    pub paused: Arc<AtomicBool>,
    queue: Arc<Queue>,
    tqueue: Arc<TQueue>,
    /// Çeviri yavaşsa (büyük model / zayıf GPU) kısmi sonuçlar atlanır; GPU
    /// yalnızca final cümlelere ayrılır.
    slow: Arc<AtomicBool>,
}

impl Pipeline {
    pub fn new() -> Self {
        Self {
            stop: Arc::new(AtomicBool::new(false)),
            whisper_ready: Arc::new(AtomicBool::new(false)),
            llama_ready: Arc::new(AtomicBool::new(false)),
            vad_ready: Arc::new(AtomicBool::new(false)),
            paused: Arc::new(AtomicBool::new(false)),
            queue: Arc::new(Queue::default()),
            tqueue: Arc::new(TQueue::default()),
            slow: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
        if paused {
            self.queue.clear();
            self.tqueue.clear();
        }
    }

    pub fn start(
        &self,
        shared: Shared,
        settings: Arc<Mutex<Settings>>,
        emit: EmitFn,
        log: LogFn,
        sep_model: PathBuf,
    ) {
        {
            let (queue, stop, paused) = (Arc::clone(&self.queue), Arc::clone(&self.stop), Arc::clone(&self.paused));
            std::thread::spawn(move || segmenter(shared, queue, stop, paused));
        }
        {
            let queue = Arc::clone(&self.queue);
            let tq = Arc::clone(&self.tqueue);
            let stop = Arc::clone(&self.stop);
            let wr = Arc::clone(&self.whisper_ready);
            let lr = Arc::clone(&self.llama_ready);
            let vr = Arc::clone(&self.vad_ready);
            let (settings, emit, log) = (Arc::clone(&settings), Arc::clone(&emit), Arc::clone(&log));
            let slow = Arc::clone(&self.slow);
            let paused = Arc::clone(&self.paused);
            std::thread::spawn(move || worker(queue, tq, stop, settings, emit, log, wr, lr, vr, slow, paused, sep_model));
        }
        let tq = Arc::clone(&self.tqueue);
        let stop = Arc::clone(&self.stop);
        let lr = Arc::clone(&self.llama_ready);
        let slow = Arc::clone(&self.slow);
        let paused = Arc::clone(&self.paused);
        std::thread::spawn(move || translator(tq, stop, settings, emit, log, lr, slow, paused));
    }
}

fn rms(x: &[f32]) -> f32 {
    if x.is_empty() {
        return 0.0;
    }
    (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt()
}

/// Son 2 saniyenin en sessiz noktasını bulur (zorunlu kesim için).
fn quietest_cut(seg: &[f32]) -> usize {
    let start = seg.len().saturating_sub(TARGET_RATE as usize * 2).max(seg.len() / 2);
    let win = 800;
    let mut best = (f32::MAX, seg.len());
    let mut i = start;
    while i + win <= seg.len() {
        let r = rms(&seg[i..i + win]);
        if r < best.0 {
            best = (r, i + win / 2);
        }
        i += 400;
    }
    best.1
}

fn segmenter(shared: Shared, queue: Arc<Queue>, stop: Arc<AtomicBool>, paused: Arc<AtomicBool>) {
    let mut local: Vec<f32> = Vec::new();
    let mut pre: VecDeque<f32> = VecDeque::new();
    let mut seg: Vec<f32> = Vec::new();
    let mut active = false;
    let mut silence_ms = 0u32;
    let mut voiced = 0usize;
    let mut seg_id = 0u64;
    let mut last_sub = Instant::now();
    let mut partial_voiced = 0usize; // son kısmi istendiğindeki konuşma miktarı (0 = hiç yok)
    let mut partial_len = 0usize; // son kısmi işin örnek sayısı

    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(30));
        if paused.load(Ordering::Relaxed) {
            shared.lock().unwrap().clear();
            local.clear();
            pre.clear();
            seg.clear();
            active = false;
            continue;
        }
        {
            let mut b = shared.lock().unwrap();
            local.append(&mut b);
        }
        let mut off = 0;
        while local.len() - off >= FRAME {
            let frame = &local[off..off + FRAME];
            off += FRAME;
            let is_voiced = rms(frame) > SPEECH_RMS;

            if !active {
                pre.extend(frame.iter().copied());
                while pre.len() > PRE_ROLL {
                    pre.pop_front();
                }
                if is_voiced {
                    active = true;
                    seg_id += 1;
                    seg.clear();
                    seg.extend(pre.drain(..));
                    silence_ms = 0;
                    voiced = FRAME;
                    partial_voiced = 0;
                    last_sub = Instant::now();
                }
                continue;
            }

            seg.extend_from_slice(frame);
            if is_voiced {
                silence_ms = 0;
                voiced += FRAME;
            } else {
                silence_ms += 20;
            }

            let ended = silence_ms >= END_SILENCE_MS;
            if ended || seg.len() >= MAX_SEG_SAMPLES {
                let mut carry = Vec::new();
                let reuse_len = if ended
                    && partial_voiced > 0
                    && voiced.saturating_sub(partial_voiced) < REUSE_MAX_NEW_SAMPLES
                {
                    partial_len
                } else {
                    0
                };
                if ended {
                    // Sondaki uzun sessizliği kırp (200 ms bırak).
                    let trim = ((silence_ms.saturating_sub(200)) as usize) * TARGET_RATE as usize / 1000;
                    seg.truncate(seg.len().saturating_sub(trim));
                } else {
                    let cut = quietest_cut(&seg);
                    carry = seg.split_off(cut);
                }
                if voiced >= MIN_VOICED_SAMPLES {
                    queue.push(Job {
                        seg_id,
                        pcm: std::mem::take(&mut seg),
                        is_final: true,
                        created: Instant::now(),
                        reuse_len,
                    });
                }
                seg.clear();
                partial_voiced = 0;
                if carry.is_empty() {
                    active = false;
                } else {
                    seg_id += 1;
                    voiced = carry.len();
                    seg = carry;
                    silence_ms = 0;
                    last_sub = Instant::now();
                }
            } else if last_sub.elapsed() >= PARTIAL_EVERY
                && seg.len() >= PARTIAL_MIN_SAMPLES
                && voiced >= MIN_VOICED_SAMPLES
            {
                queue.push(Job {
                    seg_id,
                    pcm: seg.clone(),
                    is_final: false,
                    created: Instant::now(),
                    reuse_len: 0,
                });
                partial_voiced = voiced;
                partial_len = seg.len();
                last_sub = Instant::now();
            }
        }
        local.drain(..off);
    }
}

struct Transcript {
    language: String,
    text: String,
    no_speech: f32,
    /// Whisper'ın ortalama token log-olasılığı (0'a yakın = emin; < -1 = şüpheli).
    avg_logprob: f32,
}

/// Son güvenilir sonuçların çoğunluk dili (en az 3 oy), yoksa None.
fn sticky_lang(recent: &VecDeque<&'static str>) -> Option<&'static str> {
    let mut best: Option<(&'static str, usize)> = None;
    for &c in recent {
        let n = recent.iter().filter(|&&x| x == c).count();
        if n >= 3 && best.map_or(true, |(_, bn)| n > bn) {
            best = Some((c, n));
        }
    }
    best.map(|(c, _)| c)
}

/// Otomatik dil algılama kısa/gürültülü parçalarda sık yanılır (şarkıda "fr",
/// "es" gibi). Baskın dilden farklı çıkarsa, o dile zorlayıp tekrar dener ve
/// daha emin olan sonucu seçer.
fn transcribe_sticky(pcm: &[f32], s: &Settings, sticky: Option<&str>) -> Result<Transcript, String> {
    let first = transcribe(pcm, s)?;
    let Some(st) = sticky.filter(|_| s.source_lang == "auto") else {
        return Ok(first);
    };
    if langs::code_from_name(&first.language) == Some(st) {
        return Ok(first);
    }
    let short = pcm.len() < 4 * TARGET_RATE as usize;
    if !(short || first.avg_logprob < -0.8) {
        return Ok(first);
    }
    let mut forced = s.clone();
    forced.source_lang = st.to_string();
    match transcribe(pcm, &forced) {
        Ok(second) if !is_junk(&second.text) && second.avg_logprob > first.avg_logprob => Ok(second),
        _ => Ok(first),
    }
}

fn worker(
    queue: Arc<Queue>,
    tq: Arc<TQueue>,
    stop: Arc<AtomicBool>,
    settings: Arc<Mutex<Settings>>,
    emit: EmitFn,
    log: LogFn,
    whisper_ready: Arc<AtomicBool>,
    llama_ready: Arc<AtomicBool>,
    vad_ready: Arc<AtomicBool>,
    slow: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    sep_model: PathBuf,
) {
    // Vokal ayırıcı: ayar açıksa başlangıçta yüklenip ısıtılır, yoksa ilk ihtiyaçta.
    let mut separator: Option<Separator> = None;
    let mut sep_failed = false;
    let load_separator = |separator: &mut Option<Separator>, failed: &mut bool, log: &LogFn| {
        // Model henüz indirilmediyse "başarısız" saymayız; indirilince ilk ihtiyaçta yüklenir.
        if separator.is_some() || *failed || !sep_model.exists() {
            return;
        }
        let t = Instant::now();
        match Separator::load(&sep_model) {
            Ok(mut s) => {
                let _ = s.vocals(&vec![0.01f32; TARGET_RATE as usize]); // ısınma
                log(&format!(
                    "vokal ayırıcı hazır: {} ({} ms)",
                    s.provider,
                    t.elapsed().as_millis()
                ));
                *separator = Some(s);
            }
            Err(e) => {
                log(&format!("vokal ayırıcı yüklenemedi: {e}"));
                *failed = true;
            }
        }
    };
    if settings.lock().unwrap().use_separation {
        load_separator(&mut separator, &mut sep_failed, &log);
    }

    let mut last_final = String::new();
    let mut last_final_at = Instant::now();
    let mut last_partial: Option<Partial> = None;
    let mut recent_langs: VecDeque<&'static str> = VecDeque::new();
    let mut whisper_slow = false;

    while let Some(job) = queue.pop(&stop) {
        if !whisper_ready.load(Ordering::Relaxed) || paused.load(Ordering::Relaxed) {
            continue;
        }
        if !job.is_final && (whisper_slow || (slow.load(Ordering::Relaxed) && llama_ready.load(Ordering::Relaxed))) {
            continue;
        }
        // Geliştirme: ECHOSUB_DUMP_DIR verilirse her final parçanın sesini WAV olarak kaydet.
        if job.is_final {
            if let Ok(dir) = std::env::var("ECHOSUB_DUMP_DIR") {
                let _ = std::fs::create_dir_all(&dir);
                let _ = std::fs::write(
                    std::path::Path::new(&dir).join(format!("seg_{:04}.wav", job.seg_id)),
                    encode_wav(&job.pcm),
                );
            }
        }
        let q_ms = job.created.elapsed().as_millis() as u64;
        let mut s = settings.lock().unwrap().clone();
        s.use_vad = s.use_vad && vad_ready.load(Ordering::Relaxed);
        if is_stale(job.created.elapsed(), s.max_caption_age_secs, !queue.is_empty()) {
            if job.is_final {
                log(&format!("bayat parça atıldı #{} ({q_ms} ms)", job.seg_id));
                emit(&Caption::cleared(job.seg_id, true));
            }
            continue;
        }

        let t_w = Instant::now();
        let reused = reusable_partial(&last_partial, &job);
        let sticky = sticky_lang(&recent_langs);
        let was_reused = reused.is_some();
        let (mut text, mut language, mut no_speech, mut lp, mut w_ms) = match reused {
            Some(p) => (p.text, p.language, p.no_speech, p.lp, 0),
            None => match transcribe_sticky(&job.pcm, &s, sticky) {
                Ok(t) => {
                    let text = collapse_repeats(&t.text.split_whitespace().collect::<Vec<_>>().join(" "));
                    if !job.is_final {
                        last_partial = Some(Partial {
                            seg_id: job.seg_id,
                            len: job.pcm.len(),
                            text: text.clone(),
                            language: t.language.clone(),
                            no_speech: t.no_speech,
                            lp: t.avg_logprob,
                        });
                    }
                    (text, t.language, t.no_speech, t.avg_logprob, t_w.elapsed().as_millis() as u64)
                }
                Err(e) => {
                    log(&format!("whisper hatası: {e}"));
                    continue;
                }
            },
        };

        if !was_reused {
            whisper_slow = w_ms > WHISPER_SLOW_MS;
        }

        // Yedek: parça okunamadıysa (boş/uydurma/düşük güven) vokali ayırıp yeniden dene.
        // Yalnızca final parçalarda; normal konuşmada hiç devreye girmez.
        if job.is_final
            && !was_reused
            && s.use_separation
            && job.pcm.len() >= TARGET_RATE as usize * 6 / 5
            && (is_junk(&text) || no_speech > 0.5 || lp < -0.9)
        {
            if s.use_separation && separator.is_none() {
                load_separator(&mut separator, &mut sep_failed, &log);
            }
            if let Some(sp) = separator.as_mut() {
                let t_s = Instant::now();
                match sp.vocals(&job.pcm).and_then(|v| transcribe_sticky(&v, &s, sticky)) {
                    Ok(t2) => {
                        let text2 =
                            collapse_repeats(&t2.text.split_whitespace().collect::<Vec<_>>().join(" "));
                        let better = !is_junk(&text2)
                            && t2.no_speech <= 0.7
                            && (is_junk(&text) || t2.avg_logprob > lp);
                        log(&format!(
                            "vokal ayırma #{}: {} ms, {}",
                            job.seg_id,
                            t_s.elapsed().as_millis(),
                            if better { "daha iyi sonuç" } else { "kazanç yok" }
                        ));
                        if better {
                            text = text2;
                            language = t2.language;
                            no_speech = t2.no_speech;
                            lp = t2.avg_logprob;
                        }
                        w_ms += t_s.elapsed().as_millis() as u64;
                    }
                    Err(e) => log(&format!("vokal ayırma hatası: {e}")),
                }
            }
        }

        if is_junk(&text) || no_speech > 0.7 {
            if job.is_final {
                emit(&Caption::cleared(job.seg_id, true));
            }
            continue;
        }
        if job.is_final && text == last_final && last_final_at.elapsed() < Duration::from_secs(10) {
            emit(&Caption::cleared(job.seg_id, true));
            continue;
        }
        let code = langs::code_from_name(&language);
        if job.is_final {
            last_final = text.clone();
            last_final_at = Instant::now();
            if let (Some(c), true) = (code, lp > -0.8) {
                recent_langs.push_back(c);
                if recent_langs.len() > 6 {
                    recent_langs.pop_front();
                }
            }
        }

        match code {
            Some(c) if c != s.target_lang && llama_ready.load(Ordering::Relaxed) => tq.push(TJob {
                seg_id: job.seg_id,
                is_final: job.is_final,
                created: job.created,
                text,
                code: c,
                q_ms,
                w_ms,
                lp,
            }),
            // Çeviri gerekmiyor ya da yapılamıyor: orijinali hemen göster.
            _ => emit(&Caption {
                seg_id: job.seg_id,
                is_final: job.is_final,
                clear: false,
                lang: code.map(str::to_string).unwrap_or(language),
                original: text,
                translated: None,
                latency_ms: job.created.elapsed().as_millis() as u64,
                timing: format!("q={q_ms} w={w_ms} t=0 lp={lp:.2}"),
            }),
        }
    }
}

fn translator(
    tq: Arc<TQueue>,
    stop: Arc<AtomicBool>,
    settings: Arc<Mutex<Settings>>,
    emit: EmitFn,
    log: LogFn,
    llama_ready: Arc<AtomicBool>,
    slow: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
) {
    let mut cache: VecDeque<(String, String, String, String)> = VecDeque::new(); // metin, kaynak, hedef, çeviri
    let mut ema_ms = 0.0f64; // ölçülen çeviri süresinin üstel ortalaması

    while let Some(j) = tq.pop(&stop) {
        // Arkasında final bekliyorsa bu kısmi sonuç bayat; çevirmeye değmez.
        if paused.load(Ordering::Relaxed) || (!j.is_final && tq.has_final()) {
            continue;
        }
        let (dst, max_age) = {
            let s = settings.lock().unwrap();
            (s.target_lang.clone(), s.max_caption_age_secs)
        };
        if is_stale(j.created.elapsed(), max_age, !tq.is_empty()) {
            if j.is_final {
                log(&format!("bayat çeviri atıldı #{} ({} ms)", j.seg_id, j.created.elapsed().as_millis()));
                emit(&Caption::cleared(j.seg_id, true));
            }
            continue;
        }
        let mut translated = None;
        let t0 = Instant::now();
        if j.code != dst && llama_ready.load(Ordering::Relaxed) {
            translated = cache
                .iter()
                .find(|(t, a, b, _)| *t == j.text && a == j.code && *b == dst)
                .map(|(.., r)| r.clone());
            if translated.is_none() {
                match translate(&j.text, j.code, &dst) {
                    Ok(r) if !r.is_empty() => {
                        ema_ms = if ema_ms == 0.0 {
                            t0.elapsed().as_millis() as f64
                        } else {
                            0.6 * ema_ms + 0.4 * t0.elapsed().as_millis() as f64
                        };
                        // Histerezis: 450 ms üstünde yavaş mod, 300 ms altında normal.
                        if ema_ms > 450.0 {
                            slow.store(true, Ordering::Relaxed);
                        } else if ema_ms < 300.0 {
                            slow.store(false, Ordering::Relaxed);
                        }
                        cache.push_back((j.text.clone(), j.code.to_string(), dst.clone(), r.clone()));
                        if cache.len() > 16 {
                            cache.pop_front();
                        }
                        translated = Some(r);
                    }
                    Ok(_) => {}
                    Err(e) => log(&format!("çeviri hatası: {e}")),
                }
            }
        }
        let t_ms = t0.elapsed().as_millis() as u64;
        emit(&Caption {
            seg_id: j.seg_id,
            is_final: j.is_final,
            clear: false,
            lang: j.code.to_string(),
            original: j.text,
            translated,
            latency_ms: j.created.elapsed().as_millis() as u64,
            timing: format!("q={} w={} t={} lp={:.2}", j.q_ms, j.w_ms, t_ms, j.lp),
        });
    }
}

/// İlk gerçek istek GPU gölgelendirici/önbellek ısınması yüzünden yavaş olur;
/// sunucular hazır olunca boş bir istekle ısıtılır.
pub fn warmup_whisper() {
    let mut seed = 12345u32;
    let noise: Vec<f32> = (0..TARGET_RATE as usize * 2)
        .map(|_| {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            ((seed >> 16) as f32 / 65535.0 - 0.5) * 0.02
        })
        .collect();
    let mut s = Settings::default();
    s.use_vad = false;
    let _ = transcribe(&noise, &s);
    let _ = transcribe(&noise, &s);
}

pub fn warmup_llama() {
    let _ = translate("Hello, how are you today?", "en", "tr");
    let _ = translate("Good morning.", "en", "tr");
}

/// Whisper (ve çeviri modeli) bazen aynı bloğu defalarca tekrarlayan bir döngüye
/// girer ("一緒に一緒に…", "hep birlikte, hep birlikte, …"). Bir bloğun art arda
/// çok kez tekrarını tek kopyaya indirir; boşluksuz diller (CJK) için de çalışır.
fn collapse_repeats(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let mut out: Vec<char> = Vec::with_capacity(c.len());
    let mut i = 0;
    while i < c.len() {
        let mut collapsed = false;
        for p in 1..=30usize {
            if i + 2 * p > c.len() {
                break;
            }
            let min_repeats = if p <= 2 { 6 } else { 4 };
            let mut k = 1;
            while i + (k + 1) * p <= c.len() && c[i..i + p] == c[i + k * p..i + (k + 1) * p] {
                k += 1;
            }
            if k >= min_repeats {
                out.extend_from_slice(&c[i..i + p]);
                i += k * p;
                collapsed = true;
                break;
            }
        }
        if !collapsed {
            out.push(c[i]);
            i += 1;
        }
    }
    out.into_iter().collect()
}

const JUNK_SUBSTR: &[&str] = &[
    "amara.org", "thanks for watching", "thank you for watching", "subscribe", "подпишись",
    "подписывайтесь", "dimatorzok",
    "altyazı m.k", "izlediğiniz için teşekkür", "продолжение следует",
    "субтитры", "ご視聴ありがとうございました", "チャンネル登録", "字幕",
];

fn is_junk(t: &str) -> bool {
    let t = t.trim();
    if t.is_empty() {
        return true;
    }
    let low = t.to_lowercase();
    if (low.starts_with('[') && low.ends_with(']')) || (low.starts_with('(') && low.ends_with(')')) {
        return true;
    }
    if t.chars().all(|c| !c.is_alphanumeric()) {
        return true; // yalnızca ♪ ... gibi işaretler
    }
    if JUNK_SUBSTR.iter().any(|j| low.contains(j)) {
        return true;
    }
    // Tekrar döngüsü: aynı kelime/karakter dizisi defalarca.
    let words: Vec<&str> = t.split_whitespace().collect();
    if words.len() >= 6 {
        let mut u: Vec<&str> = words.clone();
        u.sort_unstable();
        u.dedup();
        if (u.len() as f32) / (words.len() as f32) < 0.25 {
            return true;
        }
    }
    let chars: Vec<char> = t.chars().filter(|c| !c.is_whitespace()).collect();
    if chars.len() >= 12 {
        let mut u = chars.clone();
        u.sort_unstable();
        u.dedup();
        if u.len() <= 3 {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------- HTTP

pub fn encode_wav(samples: &[f32]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut w = Vec::with_capacity(44 + data_len as usize);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data_len).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&TARGET_RATE.to_le_bytes());
    w.extend_from_slice(&(TARGET_RATE * 2).to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        w.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    w
}

fn transcribe(pcm: &[f32], s: &Settings) -> Result<Transcript, String> {
    let boundary = "----echosub";
    let mut body: Vec<u8> = Vec::new();
    let mut field = |name: &str, value: &str| {
        body.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n")
                .as_bytes(),
        );
    };
    field("response_format", "verbose_json");
    field("language", &s.source_lang);
    field("temperature", "0.0");
    field("temperature_inc", "0.0");
    field("suppress_nst", "true");
    field("no_language_probabilities", "true");
    field("vad", if s.use_vad { "true" } else { "false" });
    body.extend_from_slice(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"c.wav\"\r\nContent-Type: audio/wav\r\n\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(&encode_wav(pcm));
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let resp = ureq::post(&format!("http://127.0.0.1:{WHISPER_PORT}/inference"))
        .timeout(HTTP_TIMEOUT)
        .set("Content-Type", &format!("multipart/form-data; boundary={boundary}"))
        .send_bytes(&body)
        .map_err(|e| e.to_string())?
        .into_string()
        .map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_str(&resp).map_err(|e| format!("{e}: {resp}"))?;
    let segs = v["segments"].as_array().cloned().unwrap_or_default();
    let no_speech = if segs.is_empty() {
        0.0
    } else {
        segs.iter().map(|g| g["no_speech_prob"].as_f64().unwrap_or(0.0) as f32).sum::<f32>()
            / segs.len() as f32
    };
    let lps: Vec<f32> = segs
        .iter()
        .filter_map(|g| g["avg_logprob"].as_f64())
        .map(|x| x as f32)
        .filter(|x| x.is_finite())
        .collect();
    let avg_logprob = if lps.is_empty() { 0.0 } else { lps.iter().sum::<f32>() / lps.len() as f32 };
    Ok(Transcript {
        language: v["language"].as_str().unwrap_or("").to_string(),
        text: v["text"].as_str().unwrap_or("").to_string(),
        no_speech,
        avg_logprob,
    })
}

/// TranslateGemma istem biçimi (model kartındaki sohbet şablonuyla aynı).
fn translate(text: &str, src: &str, dst: &str) -> Result<String, String> {
    let (sn, dn) = (langs::display_name(src), langs::display_name(dst));
    let prompt = format!(
        "<start_of_turn>user\nYou are a professional {sn} ({src}) to {dn} ({dst}) translator. \
Your goal is to accurately convey the meaning and nuances of the original {sn} text while adhering \
to {dn} grammar, vocabulary, and cultural sensitivities.\nProduce only the {dn} translation, without \
any additional explanations or commentary. Please translate the following {sn} text into {dn}:\n\n\n\
{text}<end_of_turn>\n<start_of_turn>model\n"
    );
    let req = serde_json::json!({
        "prompt": prompt,
        // Girdiyle orantılı üst sınır: döngüye giren çeviri saniyelerce sürmesin.
        "n_predict": (text.chars().count() * 4).clamp(48, 320),
        "temperature": 0.0,
        "cache_prompt": true,
        "stop": ["<end_of_turn>"],
    });
    let resp = ureq::post(&format!("http://127.0.0.1:{LLAMA_PORT}/completion"))
        .timeout(HTTP_TIMEOUT)
        .set("Content-Type", "application/json")
        .send_string(&req.to_string())
        .map_err(|e| e.to_string())?
        .into_string()
        .map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_str(&resp).map_err(|e| e.to_string())?;
    Ok(collapse_repeats(v["content"].as_str().unwrap_or("").trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapses_cjk_loop() {
        let s = format!("おはよう。{}です", "一緒に".repeat(30));
        assert_eq!(collapse_repeats(&s), "おはよう。一緒にです");
    }

    #[test]
    fn collapses_spaced_loop() {
        let s = format!("Ben de {}", "hep birlikte, ".repeat(12));
        assert_eq!(collapse_repeats(&s), "Ben de hep birlikte, ");
    }

    #[test]
    fn leaves_normal_text_alone() {
        for s in [
            "Merhaba herkese, bugün hava çok güzel.",
            "ha ha ha",
            "これから一緒に日本の文化について学びましょう。",
            "Сегодня мы поговорим о том, как работает искусственный интеллект",
        ] {
            assert_eq!(collapse_repeats(s), s);
        }
    }

    // ------------------------------------------------------------ is_junk

    #[test]
    fn junk_detects_whisper_hallucinations() {
        for s in [
            "", "   ", "♪♪♪", "...", "[Music]", "(applause)",
            "Thanks for watching!", "Altyazı M.K.", "Продолжение следует...",
            "ご視聴ありがとうございました", "aaaaaaaaaaaaaaaa",
            "la la la la la la la la la la la la",
        ] {
            assert!(is_junk(s), "çöp sayılmalı: {s:?}");
        }
    }

    #[test]
    fn junk_keeps_real_sentences() {
        for s in [
            "Merhaba, nasılsın?",
            "Today we are going to talk about captions.",
            "これから一緒に日本の文化について学びましょう。",
            "Сегодня мы поговорим о том, как работает искусственный интеллект",
        ] {
            assert!(!is_junk(s), "çöp sayılmamalı: {s:?}");
        }
    }

    // ------------------------------------------------------------ dil seçimi

    #[test]
    fn sticky_lang_needs_three_votes() {
        let mut r: VecDeque<&'static str> = VecDeque::new();
        assert_eq!(sticky_lang(&r), None);
        r.extend(["ja", "ja"]);
        assert_eq!(sticky_lang(&r), None);
        r.push_back("ja");
        assert_eq!(sticky_lang(&r), Some("ja"));
        r.extend(["fr", "fr", "fr", "fr"]);
        assert_eq!(sticky_lang(&r), Some("fr"), "çoğunluk kazanır");
    }

    // ------------------------------------------------------------ ses yardımcıları

    #[test]
    fn rms_of_constant_signal() {
        assert_eq!(rms(&[]), 0.0);
        assert!((rms(&[0.5; 100]) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn quietest_cut_finds_the_gap() {
        // 6 sn gürültülü ses, son 2 sn içinde kısa bir sessizlik.
        let n = TARGET_RATE as usize * 6;
        let mut x: Vec<f32> = (0..n).map(|i| if i % 2 == 0 { 0.4 } else { -0.4 }).collect();
        let gap = n - TARGET_RATE as usize;
        for v in &mut x[gap..gap + 3200] {
            *v = 0.0;
        }
        let cut = quietest_cut(&x);
        assert!((gap..gap + 3200).contains(&cut), "kesim sessizlikte olmalı: {cut}");
    }

    #[test]
    fn wav_header_and_length() {
        let w = encode_wav(&[0.0, 1.0, -1.0, 2.0]);
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(&w[8..16], b"WAVEfmt ");
        assert_eq!(w.len(), 44 + 8);
        assert_eq!(u32::from_le_bytes(w[24..28].try_into().unwrap()), TARGET_RATE);
        assert_eq!(i16::from_le_bytes(w[46..48].try_into().unwrap()), 32767);
        assert_eq!(i16::from_le_bytes(w[50..52].try_into().unwrap()), 32767, "taşma kırpılır");
    }

    // ------------------------------------------------------------ kuyruklar

    fn job(id: u64, is_final: bool) -> Job {
        Job { seg_id: id, pcm: vec![0.0; 10], is_final, created: Instant::now(), reuse_len: 0 }
    }

    #[test]
    fn final_reuses_a_partial_only_if_it_is_the_last_one_requested() {
        let p = |seg_id, len| {
            Some(Partial { seg_id, len, text: "x".into(), language: "en".into(), no_speech: 0.0, lp: 0.0 })
        };
        let fin = |id, reuse_len| Job { reuse_len, ..job(id, true) };
        assert!(reusable_partial(&p(3, 1600), &fin(3, 1600)).is_some());
        // Son kısmi iş (2400 örnek) kuyrukta düştü; elimizde daha eski sonuç var: son kelimeler eksik olur.
        assert!(reusable_partial(&p(3, 1600), &fin(3, 2400)).is_none());
        assert!(reusable_partial(&p(2, 1600), &fin(3, 1600)).is_none(), "başka segment");
        assert!(reusable_partial(&p(3, 1600), &fin(3, 0)).is_none(), "yeni konuşma var");
        assert!(reusable_partial(&None, &fin(3, 1600)).is_none());
    }

    #[test]
    fn queue_keeps_only_latest_partial_but_all_finals() {
        let q = Queue::default();
        let stop = AtomicBool::new(false);
        q.push(job(1, false));
        q.push(job(1, false)); // eski kısmi düşer
        q.push(job(1, true));
        q.push(job(2, false));
        q.push(job(2, false));
        let mut order = Vec::new();
        while !q.items.lock().unwrap().is_empty() {
            let j = q.pop(&stop).unwrap();
            order.push((j.seg_id, j.is_final));
        }
        assert_eq!(order, vec![(1, true), (2, false)]);
    }

    #[test]
    fn queue_final_replaces_same_segment_final() {
        let q = Queue::default();
        q.push(job(7, true));
        q.push(job(7, true));
        assert_eq!(q.items.lock().unwrap().len(), 1);
    }

    #[test]
    fn queue_drops_the_oldest_finals_when_it_cannot_keep_up() {
        let q = Queue::default();
        for id in 1..=(MAX_PENDING_FINALS as u64 + 2) {
            q.push(job(id, true));
        }
        let ids: Vec<u64> = q.items.lock().unwrap().iter().map(|j| j.seg_id).collect();
        assert_eq!(ids, (3..=(MAX_PENDING_FINALS as u64 + 2)).collect::<Vec<_>>());
    }

    #[test]
    fn only_old_jobs_with_a_newer_one_behind_them_are_stale() {
        let s = Duration::from_secs;
        assert!(is_stale(s(20), 15.0, true));
        assert!(!is_stale(s(20), 15.0, false), "en yeni iş kalır");
        assert!(!is_stale(s(10), 15.0, true));
        assert!(!is_stale(s(999), 0.0, true), "0 = kapalı");
    }

    /// Çeviri modeli yokken (llama_ready = false) çeviri ipliğini sırayı boşaltana kadar çalıştırır.
    fn run_translator(max_age: f32, paused: bool, jobs: Vec<TJob>) -> Vec<(u64, bool)> {
        let tq = Arc::new(TQueue::default());
        for j in jobs {
            tq.push(j);
        }
        let out = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&out);
        let emit: EmitFn = Arc::new(move |c: &Caption| sink.lock().unwrap().push((c.seg_id, c.clear)));
        let log: LogFn = Arc::new(|_| {});
        let s = Settings { max_caption_age_secs: max_age, ..Settings::default() };
        let flag = |v| Arc::new(AtomicBool::new(v));
        translator(tq, flag(true), Arc::new(Mutex::new(s)), emit, log, flag(false), flag(false), flag(paused));
        let r = out.lock().unwrap().clone();
        r
    }

    fn old_final(id: u64) -> TJob {
        TJob {
            seg_id: id, is_final: true, created: Instant::now() - Duration::from_secs(20),
            text: "x".into(), code: "en", q_ms: 0, w_ms: 0, lp: 0.0,
        }
    }

    #[test]
    fn translator_removes_stale_captions_but_shows_the_newest() {
        let got = run_translator(15.0, false, vec![old_final(1), old_final(2), old_final(3)]);
        assert_eq!(got, vec![(1, true), (2, true), (3, false)]);
    }

    #[test]
    fn translator_keeps_everything_when_the_age_limit_is_off() {
        let got = run_translator(0.0, false, vec![old_final(1), old_final(2)]);
        assert_eq!(got, vec![(1, false), (2, false)]);
    }

    #[test]
    fn translator_shows_nothing_while_paused() {
        assert!(run_translator(15.0, true, vec![old_final(1)]).is_empty());
    }

    #[test]
    fn pausing_forgets_waiting_jobs() {
        let p = Pipeline::new();
        p.queue.push(job(1, true));
        p.tqueue.push(TJob {
            seg_id: 1, is_final: true, created: Instant::now(), text: "x".into(),
            code: "en", q_ms: 0, w_ms: 0, lp: 0.0,
        });
        p.set_paused(false);
        assert!(!p.queue.is_empty() && !p.tqueue.is_empty(), "devam etmek işleri silmez");
        p.set_paused(true);
        assert!(p.queue.is_empty() && p.tqueue.is_empty());
        assert!(p.paused.load(Ordering::Relaxed));
    }

    #[test]
    fn queue_pop_returns_none_when_stopped_and_empty() {
        let q = Queue::default();
        let stop = AtomicBool::new(true);
        assert!(q.pop(&stop).is_none());
    }

    #[test]
    fn tqueue_drops_stale_partials() {
        let q = TQueue::default();
        let t = |id, f| TJob {
            seg_id: id, is_final: f, created: Instant::now(), text: "x".into(),
            code: "en", q_ms: 0, w_ms: 0, lp: 0.0,
        };
        q.push(t(1, false));
        q.push(t(1, true));
        assert!(q.has_final());
        q.push(t(2, false));
        q.push(t(2, false));
        assert_eq!(q.items.lock().unwrap().len(), 2);
    }
}
