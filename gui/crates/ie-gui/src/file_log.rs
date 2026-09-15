// SPDX-FileCopyrightText: 2026 Javiju
//
// SPDX-License-Identifier: MIT

//! Log de sesión en fichero, junto al ejecutable (`ie-repack.log`).
//!
//! La GUI ya muestra un registro en pantalla; este módulo lo duplica a disco
//! para que un fallo se pueda reportar con contexto (versión, entradas,
//! etapas). Siempre activo, sin interruptor: si algo sale mal, el log ya
//! está ahí. Los errores de escritura se ignoran a propósito (el log nunca
//! debe tumbar la app).

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

static FILE: OnceLock<Mutex<Option<std::fs::File>>> = OnceLock::new();

fn slot() -> &'static Mutex<Option<std::fs::File>> {
    FILE.get_or_init(|| Mutex::new(None))
}

/// Ruta del log (junto al ejecutable; si no se puede, junto al CWD).
pub fn path() -> PathBuf {
    let base = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("ie-repack.log")
}

/// Abre el fichero (append) y escribe la cabecera de sesión. Idempotente.
pub fn init() -> PathBuf {
    let p = path();
    let mut fh = std::fs::OpenOptions::new().create(true).append(true).open(&p).ok();
    if let Some(f) = fh.as_mut() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(f, "=== sesión unix={now} app={} ===", env!("CARGO_PKG_VERSION"));
    }
    *slot().lock().unwrap() = fh;
    p
}

/// Añade una línea al log (y a nada más si aún no hay fichero).
pub fn line(s: &str) {
    if let Some(f) = slot().lock().unwrap().as_mut() {
        let _ = writeln!(f, "{s}");
        let _ = f.flush();
    }
}
