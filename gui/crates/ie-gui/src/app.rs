// SPDX-FileCopyrightText: 2026 Javiju
//
// SPDX-License-Identifier: MIT

//! Ventana principal: elegir CIA + parche, lanzar el pipeline con progreso.

use eframe::egui;
use ie_core::pipeline;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

use crate::file_log;

enum WorkerMsg {
    Progress(pipeline::StageProgress),
    Done(Result<(), String>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Flujo Galaxy: CIA japonés + parche (forzado -n) -> CCI.
    Galaxy,
    /// Flujo pack: base CIA/.3ds + carpeta con manifiesto.json y parches
    /// por fichero -> CCI. Recomendado para IE 1-2-3 ES (todo verificado).
    Pack,
    /// Flujo genérico: base CIA/CCI + cadena de parches estrictos -> CCI.
    /// Para parches sobre .3ds descifrado canónico.
    /// Experimental hasta validar con un dump real del juego en cuestión.
    Generic,
}

impl Mode {
    fn label(self) -> &'static str {
        match self {
            Mode::Galaxy => "Galaxy Supernova",
            Mode::Pack => "Pack (manifiesto)",
            Mode::Generic => "Xdelta estricto",
        }
    }

    fn tip(self) -> &'static str {
        match self {
            Mode::Galaxy => "CIA japonés de Galaxy + parche público -> .3ds",
            Mode::Pack => "Base japonesa + carpeta del pack (manifiesto.json) -> .3ds",
            Mode::Generic => "Base + cadena de parches .xdelta en orden (experimental)",
        }
    }

    fn guide(self) -> &'static [&'static str] {
        match self {
            Mode::Galaxy => &[
                "1. Elige tu CIA japonés de Galaxy Supernova.",
                "2. Elige el parche (.xdelta o el .zip del blog).",
                "3. Pulsa el botón y espera: sale un .3ds en español.",
            ],
            Mode::Pack => &[
                "1. Elige tu base japonesa (.3ds descifrado; vale el .cia).",
                "2. Elige la carpeta del pack (la que trae manifiesto.json).",
                "3. Pulsa el botón y espera: cada fichero se verifica dos veces.",
            ],
            Mode::Generic => &[
                "1. Elige tu base (.cia o .3ds).",
                "2. Añade los .xdelta en orden de aplicación.",
                "3. Opcional: pega los SHA que publique el proyecto.",
            ],
        }
    }
}

pub struct App {
    mode: Mode,
    base: Option<PathBuf>,
    base_info: String,
    base_ok: bool,
    patch: Option<PathBuf>,
    patch_info: String,
    patch_ok: bool,
    out: String,
    // Estado del modo genérico.
    g_base: Option<PathBuf>,
    g_base_info: String,
    g_base_ok: bool,
    g_patches: Vec<PathBuf>,
    g_sha: String,
    g_sha_content: String,
    // Estado del modo pack.
    m_base: Option<PathBuf>,
    m_base_info: String,
    m_base_ok: bool,
    m_pack: Option<PathBuf>,
    m_pack_info: String,
    m_pack_ok: bool,
    log_path: PathBuf,
    running: bool,
    overall: f32,
    stage_label: String,
    stage_detail: String,
    log: Vec<String>,
    result: Option<Result<String, String>>,
    cancel: Arc<AtomicBool>,
    rx: Option<mpsc::Receiver<WorkerMsg>>,
    last_logged_stage: Option<usize>,
}

/// Regla de precarga: exactamente un candidato -> se elige solo.
fn pick_single(mut files: Vec<PathBuf>) -> Option<PathBuf> {
    files.sort();
    if files.len() == 1 { files.pop() } else { None }
}

fn siblings_with_ext(dir: &Path, exts: &[&str]) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    rd.filter_map(|e| e.ok().map(|x| x.path()))
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .and_then(|e| e.to_str())
                    .map(|e| exts.iter().any(|w| w.eq_ignore_ascii_case(e)))
                    .unwrap_or(false)
        })
        .collect()
}

/// Nombre corto para la tarjeta (elide por el medio si es muy largo, para que
/// la tarjeta nunca fuerce el ancho de la ventana).
fn short_name(path: &PathBuf) -> String {
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("(nombre raro)");
    elide_middle(name, 38)
}

fn elide_middle(s: &str, max_chars: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        return s.to_owned();
    }
    let keep_end = 14;
    let keep_start = max_chars - keep_end - 3;
    format!("{}...{}", chars[..keep_start].iter().collect::<String>(), chars[chars.len() - keep_end..].iter().collect::<String>())
}

impl Default for App {
    fn default() -> Self {
        let mut app = Self {
            mode: Mode::Galaxy,
            base: None,
            base_info: "Sin seleccionar".into(),
            base_ok: false,
            patch: None,
            patch_info: "Sin seleccionar".into(),
            patch_ok: false,
            out: String::new(),
            g_base: None,
            g_base_info: "Sin seleccionar".into(),
            g_base_ok: false,
            g_patches: Vec::new(),
            g_sha: String::new(),
            g_sha_content: String::new(),
            m_base: None,
            m_base_info: "Sin seleccionar".into(),
            m_base_ok: false,
            m_pack: None,
            m_pack_info: "Sin seleccionar".into(),
            m_pack_ok: false,
            log_path: file_log::init(),
            running: false,
            overall: 0.0,
            stage_label: String::new(),
            stage_detail: String::new(),
            log: Vec::new(),
            result: None,
            cancel: Arc::new(AtomicBool::new(false)),
            rx: None,
            last_logged_stage: None,
        };
        app.push_log("Elige tu CIA japonés y el parche (.xdelta o .zip del blog).");
        // Precarga: lo que esté junto al ejecutable.
        if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())) {
            let cias = siblings_with_ext(&dir, &["cia"]);
            let patches = siblings_with_ext(&dir, &["xdelta", "zip"]);
            match pick_single(cias) {
                Some(p) => app.set_base(p),
                None => app.push_log("Junto a la app no hay un único .cia: elígelo a mano."),
            }
            match pick_single(patches) {
                Some(p) => app.set_patch(p),
                None => app.push_log("Junto a la app no hay un único parche: elígelo a mano."),
            }
        }
        app
    }
}

impl App {
    fn push_log(&mut self, line: impl Into<String>) {
        let line = line.into();
        file_log::line(&line);
        self.log.push(line);
        if self.log.len() > 400 {
            let excess = self.log.len() - 400;
            self.log.drain(..excess);
        }
    }

    fn set_base(&mut self, path: PathBuf) {
        self.base = Some(path.clone());
        self.result = None;
        match describe_cia(&path) {
            Ok(info) => {
                self.base_info = info.clone();
                self.base_ok = true;
                self.push_log(format!("Base: {} ({info})", path.display()));
                if self.out.is_empty() {
                    self.out = default_out(&path);
                }
            }
            Err(e) => {
                self.base_info = format!("No válido: {e}");
                self.base_ok = false;
                self.push_log(format!("Base rechazada ({}): {e}", path.display()));
            }
        }
    }

    fn set_patch(&mut self, path: PathBuf) {
        self.patch = Some(path.clone());
        self.result = None;
        match describe_patch(&path) {
            Ok(info) => {
                self.patch_info = info.clone();
                self.patch_ok = true;
                self.push_log(format!("Parche: {} ({info})", path.display()));
            }
            Err(e) => {
                self.patch_info = format!("No válido: {e}");
                self.patch_ok = false;
                self.push_log(format!("Parche rechazado ({}): {e}", path.display()));
            }
        }
    }

    fn set_g_base(&mut self, path: PathBuf) {
        self.g_base = Some(path.clone());
        self.result = None;
        match describe_any(&path) {
            Ok(info) => {
                self.g_base_info = info.clone();
                self.g_base_ok = true;
                self.push_log(format!("Base: {} ({info})", path.display()));
                if self.out.is_empty() {
                    self.out = default_out(&path);
                }
            }
            Err(e) => {
                self.g_base_info = format!("No válido: {e}");
                self.g_base_ok = false;
                self.push_log(format!("Base rechazada ({}): {e}", path.display()));
            }
        }
    }

    fn add_g_patches(&mut self, paths: Vec<PathBuf>) {
        for p in paths {
            if p.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("xdelta") || e.eq_ignore_ascii_case("zip")).unwrap_or(false) {
                if !self.g_patches.contains(&p) {
                    self.g_patches.push(p.clone());
                    self.push_log(format!("Parche añadido: {}", p.display()));
                }
            } else {
                self.push_log(format!("Ignorado ({}): se esperaba .xdelta / .zip", p.display()));
            }
        }
        self.g_patches.sort();
        self.result = None;
    }

    fn set_m_base(&mut self, path: PathBuf) {
        self.m_base = Some(path.clone());
        self.result = None;
        match describe_any(&path) {
            Ok(info) => {
                self.m_base_info = info.clone();
                self.m_base_ok = true;
                self.push_log(format!("Base: {} ({info})", path.display()));
                if self.out.is_empty() {
                    self.out = default_out(&path);
                }
            }
            Err(e) => {
                self.m_base_info = format!("No válido: {e}");
                self.m_base_ok = false;
                self.push_log(format!("Base rechazada ({}): {e}", path.display()));
            }
        }
    }

    fn set_m_pack(&mut self, dir: PathBuf) {
        self.m_pack = Some(dir.clone());
        self.result = None;
        match ie_core::manifest::describe(&dir) {
            Ok(info) => {
                self.m_pack_info = info.clone();
                self.m_pack_ok = true;
                self.push_log(format!("Pack: {} ({info})", dir.display()));
            }
            Err(e) => {
                self.m_pack_info = format!("No válido: {e}");
                self.m_pack_ok = false;
                self.push_log(format!("Pack rechazado ({}): {e}", dir.display()));
            }
        }
    }

    fn manifest_ready(&self) -> String {
        if !self.m_base_ok {
            return "falta una base válida".into();
        }
        if !self.m_pack_ok {
            return "falta un pack válido (carpeta con manifiesto.json)".into();
        }
        if self.out.trim().is_empty() {
            return "falta la salida".into();
        }
        String::new()
    }

    /// Rama pack: base + carpeta de pack con manifiesto. Sin temporales de
    /// parches sueltos: el manifiesto ya trae las rutas relativas.
    fn start_manifest(&mut self) {
        let problem = self.manifest_ready();
        if !problem.is_empty() {
            self.result = Some(Err(problem));
            return;
        }
        let base = self.m_base.clone().unwrap();
        let pack = self.m_pack.clone().unwrap();
        if self.out.trim().is_empty() {
            self.out = default_out(&base);
        }
        let out = PathBuf::from(self.out.trim());
        if let Some(parent) = out.parent() {
            if !parent.as_os_str().is_empty() && std::fs::create_dir_all(parent).is_err() {
                self.result = Some(Err("no se puede crear la carpeta de salida".into()));
                return;
            }
        }
        self.running = true;
        self.overall = 0.0;
        self.result = None;
        self.last_logged_stage = None;
        self.cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        let cancel = self.cancel.clone();
        self.push_log(format!("Pack {} sobre {} ...", pack.display(), base.display()));
        std::thread::spawn(move || {
            let r = pipeline::run_manifest(
                &pipeline::ManifestInputs { base, pack_dir: pack, out_cci: out, keep_work: false },
                &cancel,
                &mut |p| {
                    tx.send(WorkerMsg::Progress(p)).ok();
                },
            );
            let _ = tx.send(WorkerMsg::Done(r.map_err(|e| friendly_error(&e))));
        });
    }

    fn generic_ready(&self) -> String {
        if !self.g_base_ok {
            return "falta una base válida".into();
        }
        if self.g_patches.is_empty() {
            return "falta al menos un parche".into();
        }
        for (tag, s) in [("SHA-256", &self.g_sha), ("SHA-256 de contenido", &self.g_sha_content)] {
            let t = s.trim();
            if !t.is_empty() && !(t.len() == 64 && t.chars().all(|c| c.is_ascii_hexdigit())) {
                return format!("{tag}: deben ser 64 hex o vacío");
            }
        }
        if self.out.trim().is_empty() {
            return "falta la salida".into();
        }
        String::new()
    }

    fn start(&mut self) {
        if self.mode == Mode::Pack {
            self.start_manifest();
            return;
        }
        if self.mode == Mode::Generic {
            self.start_generic();
            return;
        }
        let (base, patch) = match (self.base.clone(), self.patch.clone()) {
            (Some(b), Some(p)) => (b, p),
            _ => return,
        };
        if self.out.trim().is_empty() {
            self.out = default_out(&base);
        }
        let out = PathBuf::from(self.out.trim());
        if let Some(parent) = out.parent() {
            if !parent.as_os_str().is_empty() && std::fs::create_dir_all(parent).is_err() {
                self.result = Some(Err("no se puede crear la carpeta de salida".into()));
                return;
            }
        }
        self.running = true;
        self.overall = 0.0;
        self.result = None;
        self.last_logged_stage = None;
        self.cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        let cancel = self.cancel.clone();
        self.push_log(format!("Procesando {} ...", base.display()));
        std::thread::spawn(move || {
            let inputs = pipeline::Inputs { base_cia: base, patch, out_3ds: out, keep_work: false };
            let r = pipeline::run(
                &inputs,
                &cancel,
                &mut |p| {
                    tx.send(WorkerMsg::Progress(p)).ok();
                },
            );
            let _ = tx.send(WorkerMsg::Done(r.map_err(|e| friendly_error(&e))));
        });
    }

    /// Rama genérica: base CIA/CCI + cadena estricta. Los .zip se resuelven a
    /// su único .xdelta en un temporal junto a la salida.
    fn start_generic(&mut self) {
        let problem = self.generic_ready();
        if !problem.is_empty() {
            self.result = Some(Err(problem));
            return;
        }
        let base = self.g_base.clone().unwrap();
        let patches_in = self.g_patches.clone();
        if self.out.trim().is_empty() {
            self.out = default_out(&base);
        }
        let out = PathBuf::from(self.out.trim());
        if let Some(parent) = out.parent() {
            if !parent.as_os_str().is_empty() && std::fs::create_dir_all(parent).is_err() {
                self.result = Some(Err("no se puede crear la carpeta de salida".into()));
                return;
            }
        }
        self.running = true;
        self.overall = 0.0;
        self.result = None;
        self.last_logged_stage = None;
        self.cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        let cancel = self.cancel.clone();
        let sha = self.g_sha.trim().to_owned();
        let sha_content = self.g_sha_content.trim().to_owned();
        self.push_log(format!("Procesando {} con {} parche(s) ...", base.display(), patches_in.len()));
        std::thread::spawn(move || {
            let r = (|| -> Result<(), ie_core::error::Error> {
                // Temporal para los .xdelta extraídos de zips.
                let pid = std::process::id();
                let nanos = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0);
                let unpack = out.parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join(format!(".ie-patches-{pid}-{nanos:x}"));
                std::fs::create_dir_all(&unpack)?;
                let mut patches = Vec::with_capacity(patches_in.len());
                for (i, p) in patches_in.iter().enumerate() {
                    let is_zip = p.extension().map(|e| e.eq_ignore_ascii_case("zip")).unwrap_or(false);
                    if is_zip {
                        let dest = unpack.join(format!("patch{i}.xdelta"));
                        ie_core::pipeline::extract_single_xdelta(p, &dest, &cancel)?;
                        patches.push(dest);
                    } else {
                        patches.push(p.clone());
                    }
                }
                let r = pipeline::run_strict(
                    &pipeline::StrictInputs {
                        base,
                        patches,
                        expected_sha256: if sha.is_empty() { None } else { Some(sha) },
                        expected_content_sha256: if sha_content.is_empty() { None } else { Some(sha_content) },
                        out_cci: out,
                        keep_work: false,
                    },
                    &cancel,
                    &mut |p| {
                        tx.send(WorkerMsg::Progress(p)).ok();
                    },
                );
                std::fs::remove_dir_all(&unpack).ok();
                r
            })();
            let _ = tx.send(WorkerMsg::Done(r.map_err(|e| friendly_error(&e))));
        });
    }

    fn pump(&mut self) {
        let mut done = None;
        // Se saca el receiver para poder mutar el resto (p.ej. push_log)
        // mientras se drena; si no hay Done se devuelve a su sitio.
        if let Some(rx) = self.rx.take() {
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    WorkerMsg::Progress(p) => {
                        self.overall = p.overall;
                        if self.last_logged_stage != Some(p.stage_idx) {
                            self.last_logged_stage = Some(p.stage_idx);
                            self.push_log(format!("Etapa {}/{}: {}", p.stage_idx + 1, p.stage_count, p.stage));
                        }
                        self.stage_label = format!("Etapa {}/{}: {}", p.stage_idx + 1, p.stage_count, p.stage);
                        self.stage_detail = if p.total > 0 {
                            format!("{} / {} MB", p.done / 1_000_000, p.total / 1_000_000)
                        } else {
                            String::new()
                        };
                    }
                    WorkerMsg::Done(r) => {
                        done = Some(r);
                        break;
                    }
                }
            }
            if done.is_none() {
                self.rx = Some(rx);
            }
        }
        if let Some(r) = done {
            self.running = false;
            self.rx = None;
            match r {
                Ok(()) => {
                    self.overall = 1.0;
                    let msg = format!("Listo: {}", self.out.trim());
                    self.push_log(msg.clone());
                    self.result = Some(Ok(msg));
                }
                Err(e) => {
                    self.push_log(format!("ERROR: {e}"));
                    self.result = Some(Err(e));
                }
            }
        }
    }
}

/// Describe base CIA o CCI: producto, título y estado de cifrado.
fn describe_any(path: &PathBuf) -> Result<String, String> {
    match ie_core::normalize::detect(path).map_err(|e| e.to_string())? {
        ie_core::normalize::InputKind::Cia => {
            let info = describe_cia(path)?;
            let layout = ie_core::cia::read_layout(path).map_err(|e| e.to_string())?;
            let c0 = layout.content(0).map_err(|e| e.to_string())?;
            let hdr = read_ncch(path, c0.offset)?;
            let state = if hdr.is_decrypted() { "descifrado" } else { "cifrado" };
            Ok(format!("CIA {info} [{state}]"))
        }
        ie_core::normalize::InputKind::Cci => {
            let parts = ie_core::cci::partitions(path).map_err(|e| e.to_string())?;
            let (p0, _) = *parts.first().ok_or("CCI sin particiones")?;
            let hdr = read_ncch(path, p0)?;
            let code = hdr.product_code();
            if code.is_empty() {
                return Err("no parece un CCI de 3DS".into());
            }
            let state = if hdr.is_decrypted() { "descifrado" } else { "cifrado" };
            Ok(format!("3ds {code}, TitleId {:016X} [{state}]", hdr.title_id()))
        }
    }
}

/// Lee TitleId y código de producto del content0 sin descifrar nada.
fn describe_cia(path: &PathBuf) -> Result<String, String> {
    let layout = ie_core::cia::read_layout(path).map_err(|e| e.to_string())?;
    let c0 = layout.content(0).map_err(|e| e.to_string())?;
    let hdr = read_ncch(path, c0.offset)?;
    let code = hdr.product_code();
    if code.is_empty() {
        return Err("no parece un CIA de 3DS".into());
    }
    let tag = if code.contains("BGSJ") {
        "Supernova"
    } else if code.contains("BGBJ") {
        "Big Bang (no probado)"
    } else if code.contains("AETJ") {
        "Inazuma Eleven 1-2-3"
    } else {
        "juego desconocido"
    };
    Ok(format!("{code} [{tag}]"))
}

fn read_ncch(path: &PathBuf, c0_off: u64) -> Result<ie_core::ncch::NcchHeader, String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    f.seek(SeekFrom::Start(c0_off)).map_err(|e| e.to_string())?;
    let mut raw = [0u8; 0x200];
    f.read_exact(&mut raw).map_err(|e| e.to_string())?;
    ie_core::ncch::NcchHeader::parse(raw).map_err(|e| e.to_string())
}

fn describe_patch(path: &PathBuf) -> Result<String, String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if meta.len() < 1024 {
        return Err("archivo demasiado pequeño para ser el parche".into());
    }
    match ext.as_str() {
        "xdelta" => Ok(format!("{:.1} MB", meta.len() as f64 / 1_000_000.0)),
        "zip" => {
            let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
            let mut zip = zip::ZipArchive::new(f).map_err(|_| "no es un .zip válido".to_string())?;
            let mut n = 0;
            for i in 0..zip.len() {
                let e = zip.by_index(i).map_err(|e| e.to_string())?;
                if e.name().to_lowercase().ends_with(".xdelta") {
                    n += 1;
                }
            }
            if n == 1 {
                Ok(format!("zip con el parche dentro ({:.0} MB)", meta.len() as f64 / 1_000_000.0))
            } else if n == 0 {
                Err("el .zip no contiene ningún .xdelta".into())
            } else {
                Err("el .zip trae varios .xdelta: extrae a mano el que corresponda".into())
            }
        }
        _ => Err("se esperaba un .xdelta o el .zip del blog".into()),
    }
}

fn default_out(base: &PathBuf) -> String {
    let stem = base.file_stem().and_then(|s| s.to_str()).unwrap_or("salida");
    let name = format!("{stem}_ESP.3ds");
    match base.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.join(name).to_string_lossy().into_owned(),
        _ => name,
    }
}

fn friendly_error(e: &ie_core::error::Error) -> String {
    use ie_core::error::Error as E;
    match e {
        E::Cancelled => "Cancelado.".into(),
        E::Verify(m) => format!("Verificación: {m}"),
        E::Xdelta(m) => format!("Parche: {m}"),
        E::Crypto(m) => format!("Descifrado: {m}"),
        E::Format(m) => format!("Archivo: {m}"),
        E::Zip(m) => format!("Zip: {m}"),
        E::Io(e) => format!("Disco: {e}"),
    }
}

impl App {
    /// Panel del modo Galaxy (dos tarjetas en horizontal).
    fn ui_galaxy(&mut self, ui: &mut egui::Ui) {
        let b_short = self.base.as_ref().map(short_name);
        let b_full = self.base.as_ref().map(|p| p.display().to_string());
        let p_short = self.patch.as_ref().map(short_name);
        let p_full = self.patch.as_ref().map(|p| p.display().to_string());
        let (b_info, b_ok) = (self.base_info.clone(), self.base_ok);
        let (p_info, p_ok) = (self.patch_info.clone(), self.patch_ok);
        let running = self.running;
        let mut pick_base = false;
        let mut pick_patch = false;
        ui.columns(2, |cols| {
            pick_base = file_card(
                &mut cols[0],
                "1. CIA japonés",
                b_short.as_deref(),
                b_full.as_deref(),
                &b_info,
                b_ok,
                "Elegir .cia...",
                running,
            );
            pick_patch = file_card(
                &mut cols[1],
                "2. Parche ES",
                p_short.as_deref(),
                p_full.as_deref(),
                &p_info,
                p_ok,
                "Elegir .xdelta/.zip...",
                running,
            );
        });
        if pick_base {
            if let Some(p) = rfd::FileDialog::new().add_filter("CIA de 3DS", &["cia"]).pick_file() {
                self.set_base(p);
            }
        }
        if pick_patch {
            if let Some(p) = rfd::FileDialog::new().add_filter("Parche", &["xdelta", "zip"]).pick_file() {
                self.set_patch(p);
            }
        }
    }

    /// Panel del modo pack: base CIA/CCI + carpeta del pack (manifiesto).
    fn ui_pack(&mut self, ui: &mut egui::Ui) {
        let b_short = self.m_base.as_ref().map(short_name);
        let b_full = self.m_base.as_ref().map(|p| p.display().to_string());
        let (b_info, b_ok) = (self.m_base_info.clone(), self.m_base_ok);
        let p_short = self.m_pack.as_ref().map(short_name);
        let p_full = self.m_pack.as_ref().map(|p| p.display().to_string());
        let (p_info, p_ok) = (self.m_pack_info.clone(), self.m_pack_ok);
        let running = self.running;
        let mut pick_base = false;
        let mut pick_pack = false;
        ui.columns(2, |cols| {
            pick_base = file_card(
                &mut cols[0],
                "1. Base (.cia/.3ds japonés)",
                b_short.as_deref(),
                b_full.as_deref(),
                &b_info,
                b_ok,
                "Elegir base...",
                running,
            );
            pick_pack = file_card(
                &mut cols[1],
                "2. Pack de traducción",
                p_short.as_deref(),
                p_full.as_deref(),
                &p_info,
                p_ok,
                "Elegir carpeta...",
                running,
            );
        });
        if pick_base {
            if let Some(p) = rfd::FileDialog::new()
                .add_filter("Base 3DS", &["cia", "3ds", "cci"])
                .pick_file()
            {
                self.set_m_base(p);
            }
        }
        if pick_pack {
            if let Some(p) = rfd::FileDialog::new().pick_folder() {
                self.set_m_pack(p);
            }
        }
    }

    /// Panel del modo genérico: base CIA/CCI + lista de parches + SHA opcional.
    fn ui_generic(&mut self, ui: &mut egui::Ui) {
        let g_short = self.g_base.as_ref().map(short_name);
        let g_full = self.g_base.as_ref().map(|p| p.display().to_string());
        let (g_info, g_ok) = (self.g_base_info.clone(), self.g_base_ok);
        let patches: Vec<String> = self.g_patches.iter().map(short_name).collect();
        let running = self.running;
        let mut pick_base = false;
        let mut add_patches = false;
        let mut clear_patches = false;
        let mut remove_idx: Option<usize> = None;
        ui.columns(2, |cols| {
            pick_base = file_card(
                &mut cols[0],
                "1. Base (.cia/.3ds)",
                g_short.as_deref(),
                g_full.as_deref(),
                &g_info,
                g_ok,
                "Elegir base...",
                running,
            );
            // Tarjeta de parches con lista ordenada y borrado por fila.
            egui::Frame::group(cols[1].style()).inner_margin(12.0).show(&mut cols[1], |ui| {
                ui.strong("2. Parches (.xdelta, en orden)");
                ui.add_space(4.0);
                if patches.is_empty() {
                    ui.weak("(sin añadir)");
                } else {
                    for (i, name) in patches.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.monospace(format!("{}. {}", i + 1, name));
                            if ui.add_enabled(!running, egui::Button::new("×").small()).clicked() {
                                remove_idx = Some(i);
                            }
                        });
                    }
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.add_enabled(!running, egui::Button::new("Añadir...")).clicked() {
                        add_patches = true;
                    }
                    if ui.add_enabled(!running && !patches.is_empty(), egui::Button::new("Limpiar")).clicked() {
                        clear_patches = true;
                    }
                });
            });
        });
        if pick_base {
            if let Some(p) = rfd::FileDialog::new()
                .add_filter("Base 3DS", &["cia", "3ds", "cci"])
                .pick_file()
            {
                self.set_g_base(p);
            }
        }
        if add_patches {
            if let Some(ps) = rfd::FileDialog::new().add_filter("Parche", &["xdelta", "zip"]).pick_files() {
                self.add_g_patches(ps);
            }
        }
        if clear_patches {
            self.g_patches.clear();
            self.result = None;
            self.push_log("Lista de parches vaciada.");
        }
        if let Some(i) = remove_idx {
            if i < self.g_patches.len() {
                let p = self.g_patches.remove(i);
                self.result = None;
                self.push_log(format!("Parche quitado: {}", p.display()));
            }
        }
        // SHA esperado (opcional) + SHA de contenido (opcional, sin cabecera).
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("SHA-256 esperado (opcional):");
            ui.add_enabled(
                !self.running,
                egui::TextEdit::singleline(&mut self.g_sha).desired_width(460.0).hint_text("64 hex del .3ds final, si el proyecto lo publica"),
            );
        });
        ui.horizontal(|ui| {
            ui.label("SHA-256 de contenido (opcional):");
            ui.add_enabled(
                !self.running,
                egui::TextEdit::singleline(&mut self.g_sha_content).desired_width(460.0).hint_text("64 hex desde el byte 0x200; vale para bases convertidas"),
            );
        });
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.pump();
        if self.running {
            ctx.request_repaint();
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            // Tipografías un punto más generosas que el defecto de egui
            // (se pedía que todo se lea sin esfuerzo).
            {
                use egui::{FontFamily, FontId, TextStyle};
                let mut style = (*ctx.style()).clone();
                for (k, size) in [
                    (TextStyle::Small, 12.0),
                    (TextStyle::Body, 15.0),
                    (TextStyle::Button, 15.0),
                    (TextStyle::Heading, 21.0),
                    (TextStyle::Monospace, 13.0),
                ] {
                    style.text_styles.insert(k, FontId::new(size, FontFamily::Proportional));
                }
                // La monoespaciada del registro, mejor en mono de verdad.
                style
                    .text_styles
                    .insert(TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace));
                ctx.set_style(style);
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.heading("IE Repack - parcheador ES");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.weak(egui::RichText::new(format!("v{}", env!("CARGO_PKG_VERSION"))).small());
                });
            });
            let subtitle = match self.mode {
                Mode::Galaxy => "Tu CIA japonés de Galaxy + el parche público -> .3ds en español listo para Azahar.",
                Mode::Pack => "Tu base japonesa + el pack de traducción -> .3ds en español listo para Azahar.",
                Mode::Generic => "Tu base + cadena de parches estrictos -> .3ds listo para Azahar.",
            };
            ui.label(subtitle);
            ui.vertical_centered(|ui| {
                ui.small("No necesitas instalar nada más: la app trae todo lo necesario.");
            });
            ui.add_space(8.0);

            // Selector de modo: botones, no desplegable. La fila lleva su
            // propio acento (borde azul claro + texto blanco frío).
            ui.horizontal(|ui| {
                ui.strong("Modo:");
                let before = self.mode;
                ui.scope(|ui| {
                    let vis = &mut ui.style_mut().visuals;
                    vis.selection.bg_fill = egui::Color32::from_rgb(33, 84, 136);
                    vis.selection.stroke = egui::Stroke::new(
                        1.5_f32,
                        egui::Color32::from_rgb(150, 195, 240),
                    );
                    vis.widgets.active.fg_stroke.color = egui::Color32::from_rgb(232, 240, 248);
                    for m in [Mode::Galaxy, Mode::Pack, Mode::Generic] {
                        ui.selectable_value(&mut self.mode, m, m.label())
                            .on_hover_text(m.tip());
                    }
                });
                if before != self.mode {
                    // Cambiar de modo no mezcla estados: resetea progreso/resultado.
                    self.overall = 0.0;
                    self.result = None;
                    self.stage_label.clear();
                    self.stage_detail.clear();
                }
            });
            ui.add_space(4.0);

            match self.mode {
                Mode::Galaxy => self.ui_galaxy(ui),
                Mode::Pack => self.ui_pack(ui),
                Mode::Generic => self.ui_generic(ui),
            }
            ui.add_space(8.0);

            // Salida. El botón se coloca primero (derecha) para que el campo
            // de texto, por largo que sea, nunca lo empuje fuera: era el
            // desborde reportado (Examinar... en x=837 con ventana de 780).
            egui::Frame::group(ui.style()).inner_margin(10.0).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.strong("3. Guardar .3ds en:");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Examinar...").clicked() && !self.running {
                            let suggest = match self.mode {
                                Mode::Galaxy => "galaxy_esp.3ds",
                                Mode::Pack => "juego_esp.3ds",
                                Mode::Generic => "juego_parcheado.3ds",
                            };
                            if let Some(p) = rfd::FileDialog::new()
                                .add_filter("Imagen 3DS", &["3ds"])
                                .set_file_name(suggest)
                                .save_file()
                            {
                                self.out = p.to_string_lossy().into_owned();
                            }
                        }
                        let w = (ui.available_width() - ui.spacing().item_spacing.x).max(80.0);
                        ui.add_enabled(!self.running, egui::TextEdit::singleline(&mut self.out).desired_width(w));
                    });
                });
            });
            ui.add_space(8.0);

            // Acción + progreso.
            let (ready, action) = match self.mode {
                Mode::Galaxy => (
                    self.base_ok && self.patch_ok && !self.out.trim().is_empty() && !self.running,
                    "Crear .3ds en español",
                ),
                Mode::Pack => (
                    self.m_base_ok && self.m_pack_ok && self.manifest_ready().is_empty() && !self.running,
                    "Aplicar pack",
                ),
                Mode::Generic => (
                    self.g_base_ok && !self.g_patches.is_empty() && self.generic_ready().is_empty() && !self.running,
                    "Aplicar parches",
                ),
            };
            ui.horizontal(|ui| {
                let btn = egui::Button::new(action).min_size(egui::vec2(220.0, 38.0));
                // Cuando está listo, el botón se viste de gala.
                let btn = if ready {
                    btn.fill(egui::Color32::from_rgb(27, 94, 32)).stroke(egui::Stroke::new(
                        1.0_f32,
                        egui::Color32::from_rgb(102, 187, 106),
                    ))
                } else {
                    btn
                };
                if ui.add_enabled(ready, btn).clicked() {
                    self.start();
                }
                if self.running && ui.button("Cancelar").clicked() {
                    self.cancel.store(true, Ordering::Relaxed);
                    self.push_log("Cancelando... (termina la etapa actual)");
                }
            });
            if self.running || self.overall > 0.0 {
                ui.label(&self.stage_label);
                ui.add(egui::ProgressBar::new(self.overall).show_percentage().desired_height(20.0));
                if !self.stage_detail.is_empty() {
                    ui.monospace(&self.stage_detail);
                }
            }
            match &self.result {
                Some(Ok(msg)) => {
                    let shown = elide_middle(msg, 80);
                    ui.colored_label(egui::Color32::GREEN, format!("OK: {shown}")).on_hover_text(msg);
                    if ui.button("Mostrar en la carpeta").clicked() {
                        // Abrir la carpeta contenedora (abrir el .3ds directo
                        // delegaría al programa asociado, que no es lo pedido).
                        let p = PathBuf::from(self.out.trim());
                        let dir = p.parent().filter(|d| !d.as_os_str().is_empty()).map(|d| d.to_path_buf()).unwrap_or_else(|| PathBuf::from("."));
                        if open::that_detached(&dir).is_err() {
                            self.push_log(format!("No se pudo abrir la carpeta: {}", dir.display()));
                        }
                    }
                }
                Some(Err(msg)) => {
                    ui.colored_label(egui::Color32::LIGHT_RED, format!("ERROR: {}", elide_middle(msg, 80)))
                        .on_hover_text(msg);
                }
                None => {}
            }
            ui.add_space(4.0);

            // Mini-guía del modo activo.
            ui.collapsing("¿Cómo se usa?", |ui| {
                for line in self.mode.guide() {
                    ui.label(*line);
                }
            });
            ui.add_space(4.0);

            // Log.
            ui.collapsing("Registro", |ui| {
                egui::ScrollArea::vertical().max_height(130.0).stick_to_bottom(true).show(ui, |ui| {
                    for line in &self.log {
                        ui.monospace(line);
                    }
                });
            });
            ui.add_space(4.0);

            // Pie: proyecto de hobby + dónde llorar si falla.
            ui.separator();
            ui.vertical_centered(|ui| {
                ui.small("Proyecto de hobby hecho con amor por un fan (Javiju555). Puede fallar: si algo sale mal, adjunta el log");
                ui.small("(ie-repack.log, junto al programa) al abrir un issue o avisa en el hilo de la traducción.");
            });
            ui.horizontal(|ui| {
                if ui.small_button("Abrir carpeta del log").clicked() {
                    let dir = self
                        .log_path
                        .parent()
                        .filter(|d| !d.as_os_str().is_empty())
                        .map(|d| d.to_path_buf())
                        .unwrap_or_else(|| PathBuf::from("."));
                    if open::that_detached(&dir).is_err() {
                        self.push_log(format!("No se pudo abrir la carpeta: {}", dir.display()));
                    }
                }
                if ui.small_button("Abrir issues en GitHub").clicked() {
                    if open::that_detached("https://github.com/Javiju555/ie-repack/issues").is_err() {
                        self.push_log("No se pudo abrir el navegador.");
                    }
                }
            });
        });
    }
}

/// Tarjeta de selección de fichero (nombre corto + ruta completa en tooltip).
/// Devuelve true si se pulsó Elegir.
fn file_card(
    ui: &mut egui::Ui,
    title: &str,
    short: Option<&str>,
    full: Option<&str>,
    info: &str,
    ok: bool,
    button: &str,
    running: bool,
) -> bool {
    egui::Frame::group(ui.style()).inner_margin(12.0).show(ui, |ui| {
        ui.strong(title);
        ui.add_space(4.0);
        match short {
            Some(name) => {
                let label = ui.monospace(name);
                if let Some(f) = full {
                    label.on_hover_text(f);
                }
            }
            None => {
                ui.weak("(sin elegir)");
            }
        }
        let color = if ok { egui::Color32::GREEN } else { ui.style().visuals.text_color() };
        ui.colored_label(color, info);
        ui.add_space(6.0);
        ui.add_enabled(!running, egui::Button::new(button)).clicked()
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(dir: &Path, name: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, b"x").unwrap();
        p
    }

    #[test]
    fn precarga_solo_si_hay_exactamente_uno() {
        let dir = std::env::temp_dir().join(format!("ie-gui-pick-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(pick_single(vec![]).is_none());
        let a = touch(&dir, "a.cia");
        assert_eq!(pick_single(vec![a.clone()]), Some(a.clone()));
        let b = touch(&dir, "b.cia");
        assert!(pick_single(vec![a, b]).is_none());
        assert_eq!(siblings_with_ext(&dir, &["cia"]).len(), 2);
        assert_eq!(siblings_with_ext(&dir, &["CIA"]).len(), 2);
        assert!(siblings_with_ext(&dir, &["zip"]).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn elide_no_rompe_nombres_cortos() {
        assert_eq!(elide_middle("corto.cia", 38), "corto.cia");
        let largo = "Inazuma Eleven Go Galaxy - Supernova (Japan).cia";
        let corto = elide_middle(largo, 38);
        assert!(corto.chars().count() <= 38);
        assert!(corto.contains("..."));
        assert!(corto.ends_with("(Japan).cia"));
    }

    /// La UI completa con rutas largas cabe en 780 px en ambos modos: ningún
    /// nodo visible puede salirse por la derecha (este era el bug reportado).
    #[test]
    fn layout_cabe_en_ventana() {
        use egui_kittest::kittest::Queryable;
        fn check(app: App, what: &str) {
            let h = egui_kittest::Harness::new_eframe(|_cc| app);
            let mut h = h;
            h.set_size(egui::vec2(780.0, 600.0));
            h.run();
            let mut bad = Vec::new();
            for n in h.query_all_by(|_| true) {
                if n.is_hidden() {
                    continue;
                }
                if let Some(r) = n.bounding_box() {
                    if r.x1 > 780.0 + 1.0 {
                        bad.push(format!(
                            "{:?} label={:?} value={:?} x0={:.0} x1={:.0}",
                            n.role(),
                            n.label(),
                            n.value(),
                            r.x0,
                            r.x1
                        ));
                    }
                }
            }
            assert!(bad.is_empty(), "{what}: widgets fuera: {bad:?}");
        }

        // Estado Galaxy completado, rutas largas.
        let mut g = App::default();
        g.base = Some(PathBuf::from("/home/javiju/Juegos/IE-Galaxy-ES/Inazuma Eleven Go Galaxy - Supernova (Japan).cia"));
        g.base_info = "CTR-P-BGSJ [Supernova]".into();
        g.base_ok = true;
        g.patch = Some(PathBuf::from("/home/javiju/Juegos/IE-Galaxy-ES/IEGOGalaxySN parche 1.0.5.zip"));
        g.patch_info = "zip con el parche dentro (1384 MB)".into();
        g.patch_ok = true;
        g.out = "/home/javiju/Juegos/IE-Galaxy-ES/Inazuma Eleven Go Galaxy - Supernova (Japan)_ESP.3ds".into();
        g.overall = 1.0;
        g.stage_label = "Etapa 7/7: Completado".into();
        g.result = Some(Ok("Listo: /home/javiju/Juegos/IE-Galaxy-ES/x.3ds".into()));
        for i in 0..6 {
            g.push_log(format!("Línea {i} /home/javiju/Juegos/IE-Galaxy-ES/Inazuma Eleven Go Galaxy - Supernova (Japan).cia"));
        }
        check(g, "galaxy");

        // Estado genérico con cadena de 2 parches y SHA.
        let mut x = App::default();
        x.mode = Mode::Generic;
        x.g_base = Some(PathBuf::from("/home/javiju/Juegos/IE123/Inazuma Eleven 1-2-3 - Endou Mamoru Densetsu.3ds"));
        x.g_base_info = "3ds CTR-P-AETJ, TitleId 00040000000EDF00 [descifrado]".into();
        x.g_base_ok = true;
        x.g_patches = vec![
            PathBuf::from("/home/javiju/Juegos/IE123/inazuma123-es-v1.xdelta"),
            PathBuf::from("/home/javiju/Juegos/IE123/inazuma123-es-v1.1-update.xdelta"),
        ];
        x.g_sha = "aa5a9f6c5da4a2a98eb5dde97ceba51fa350cbff6f406893d5e08d515b9fd1e9".into();
        x.out = "/home/javiju/Juegos/IE123/inazuma123_es_v1.1.3ds".into();
        x.overall = 0.6;
        x.stage_label = "Etapa 4/6: Aplicando parches".into();
        x.stage_detail = "1200 / 2000 MB".into();
        check(x, "generico");

        // Estado pack con base larga y pack largo.
        let mut m = App::default();
        m.mode = Mode::Pack;
        m.m_base = Some(PathBuf::from("/home/javiju/Juegos/IE123/Inazuma Eleven 1-2-3 - Endou Mamoru Densetsu.3ds"));
        m.m_base_info = "3ds CTR-P-AETJ, TitleId 00040000000EDF00 [descifrado]".into();
        m.m_base_ok = true;
        m.m_pack = Some(PathBuf::from("/home/javiju/Juegos/IE123/paquete_v55_con_un_nombre_muy_largo_para_probar"));
        m.m_pack_info = "v57-interna · 200 ficheros · base: Inazuma Eleven 1·2·3!! Endō Mamoru Densetsu".into();
        m.m_pack_ok = true;
        m.out = "/home/javiju/Juegos/IE123/inazuma123_es_v55_desde_modo_pack_con_nombre_largo.3ds".into();
        m.overall = 0.4;
        m.stage_label = "Etapa 4/7: Aplicando parches".into();
        m.stage_detail = "800 / 1700 MB".into();
        check(m, "pack");
    }
}
