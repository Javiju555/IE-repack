//! Ventana principal: elegir CIA + parche, lanzar el pipeline con progreso.

use eframe::egui;
use ie_core::pipeline;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

enum WorkerMsg {
    Progress(pipeline::StageProgress),
    Done(Result<(), String>),
}

pub struct App {
    base: Option<PathBuf>,
    base_info: String,
    base_ok: bool,
    patch: Option<PathBuf>,
    patch_info: String,
    patch_ok: bool,
    out: String,
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
            base: None,
            base_info: "Sin seleccionar".into(),
            base_ok: false,
            patch: None,
            patch_info: "Sin seleccionar".into(),
            patch_ok: false,
            out: String::new(),
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
        self.log.push(line.into());
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

    fn start(&mut self) {
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

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.pump();
        if self.running {
            ctx.request_repaint();
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(6.0);
            ui.heading("Inazuma Eleven GO Galaxy - parcheador ES");
            ui.label("Tu CIA japonés + el parche público -> .3ds en español listo para Azahar.");
            ui.label(egui::RichText::new("No necesitas instalar nada más: la app trae todo lo necesario.").small());
            ui.add_space(8.0);

            // Dos tarjetas en horizontal. (Los diálogos se abren fuera de las
            // columnas: los closures no pueden prestar `self` dos veces.)
            let b_short = self.base.as_ref().map(short_name);
            let b_full = self.base.as_ref().map(|p| p.display().to_string());
            let p_short = self.patch.as_ref().map(short_name);
            let p_full = self.patch.as_ref().map(|p| p.display().to_string());
            let (b_info, b_ok) = (self.base_info.clone(), self.base_ok);
            let (p_info, p_ok) = (self.patch_info.clone(), self.patch_ok);
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
                    self.running,
                );
                pick_patch = file_card(
                    &mut cols[1],
                    "2. Parche ES",
                    p_short.as_deref(),
                    p_full.as_deref(),
                    &p_info,
                    p_ok,
                    "Elegir .xdelta/.zip...",
                    self.running,
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
            ui.add_space(8.0);

            // Salida. El botón se coloca primero (derecha) para que el campo
            // de texto, por largo que sea, nunca lo empuje fuera: era el
            // desborde reportado (Examinar... en x=837 con ventana de 780).
            egui::Frame::group(ui.style()).inner_margin(10.0).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.strong("3. Guardar .3ds en:");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Examinar...").clicked() && !self.running {
                            if let Some(p) = rfd::FileDialog::new()
                                .add_filter("Imagen 3DS", &["3ds"])
                                .set_file_name("galaxy_esp.3ds")
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
            let ready = self.base_ok && self.patch_ok && !self.out.trim().is_empty() && !self.running;
            ui.horizontal(|ui| {
                if ui.add_enabled(ready, egui::Button::new("Crear .3ds en español").min_size(egui::vec2(220.0, 38.0))).clicked() {
                    self.start();
                }
                if self.running && ui.button("Cancelar").clicked() {
                    self.cancel.store(true, Ordering::Relaxed);
                    self.push_log("Cancelando... (termina la etapa actual)");
                }
            });
            if self.running || self.overall > 0.0 {
                ui.label(&self.stage_label);
                ui.add(egui::ProgressBar::new(self.overall).show_percentage());
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

            // Log.
            ui.collapsing("Registro", |ui| {
                egui::ScrollArea::vertical().max_height(130.0).stick_to_bottom(true).show(ui, |ui| {
                    for line in &self.log {
                        ui.monospace(line);
                    }
                });
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

    /// La UI completa con rutas largas cabe en 780 px: ningún nodo visible
    /// puede salirse por la derecha (este era el bug reportado).
    #[test]
    fn layout_cabe_en_ventana() {
        use egui_kittest::kittest::Queryable;
        let mut app = App::default();
        let base = PathBuf::from("/home/javiju/Juegos/IE-Galaxy-ES/Inazuma Eleven Go Galaxy - Supernova (Japan).cia");
        app.base = Some(base);
        app.base_info = "CTR-P-BGSJ [Supernova]".into();
        app.base_ok = true;
        app.patch = Some(PathBuf::from("/home/javiju/Juegos/IE-Galaxy-ES/IEGOGalaxySN parche 1.0.5.zip"));
        app.patch_info = "zip con el parche dentro (1384 MB)".into();
        app.patch_ok = true;
        app.out = "/home/javiju/Juegos/IE-Galaxy-ES/Inazuma Eleven Go Galaxy - Supernova (Japan)_ESP.3ds".into();
        app.overall = 1.0;
        app.stage_label = "Etapa 7/7: Completado".into();
        app.result = Some(Ok("Listo: /home/javiju/Juegos/IE-Galaxy-ES/x.3ds".into()));
        for i in 0..6 {
            app.push_log(format!("Línea {i} /home/javiju/Juegos/IE-Galaxy-ES/Inazuma Eleven Go Galaxy - Supernova (Japan).cia"));
        }
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
        assert!(bad.is_empty(), "widgets fuera de la ventana: {bad:?}");
    }
}
