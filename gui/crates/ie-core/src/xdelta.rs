//! Orquestación del binario oficial `xdelta3` (única dependencia externa).
//!
//! Por qué sidecar y no reimplementación: el parche usa compresor secundario
//! LZMA y su decode exacto lo garantiza el propio `xdelta3`, con builds
//! oficiales por SO. Todo lo específico de Nintendo (CIA/NCCH/CCI) sí es
//! nativo en este crate y está verificado byte a byte (ver tests dorados).

use crate::error::{Error, Result};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Localiza el binario: `IE_XDELTA3` > `sidecars/` junto al ejecutable > PATH.
pub fn find_binary() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("IE_XDELTA3") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Ok(p);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for sub in sidecar_subdirs() {
                let p = dir.join("sidecars").join(sub).join(exe_name());
                if p.is_file() {
                    return Ok(p);
                }
            }
            // Layout de desarrollo: <repo>/gui/sidecars/... (el exe está en
            // gui/target/debug). Se suben tres niveles hasta gui/.
            let subs = sidecar_subdirs();
            let mut d = dir.to_path_buf();
            for _ in 0..4 {
                let p = d.join("sidecars").join(&subs[0]).join(exe_name());
                if p.is_file() {
                    return Ok(p);
                }
                if !d.pop() {
                    break;
                }
            }
        }
    }
    if which_in_path(exe_name()).is_some() {
        return Ok(PathBuf::from(exe_name()));
    }
    Err(Error::Xdelta(
        "no se encontró xdelta3 (ni en sidecars ni en el PATH)".into(),
    ))
}

fn exe_name() -> &'static str {
    if cfg!(windows) {
        "xdelta3.exe"
    } else {
        "xdelta3"
    }
}

fn sidecar_subdirs() -> Vec<String> {
    let mut v = Vec::new();
    if cfg!(target_os = "windows") {
        v.push("windows-x86_64".to_string());
    } else if cfg!(target_os = "macos") {
        v.push("macos-arm64".to_string());
        v.push("macos-x86_64".to_string());
    } else {
        v.push("linux-x86_64".to_string());
    }
    v
}

fn which_in_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|d| d.join(name))
            .find(|p| p.is_file())
    })
}

/// Suma de `VCDIFF target window length` de `xdelta3 printhdrs` = tamaño
/// exacto de la salida esperada (sirve para la barra de progreso).
pub fn target_size(xdelta3: &Path, patch: &Path) -> Result<u64> {
    let out = Command::new(xdelta3)
        .arg("printhdrs")
        .arg(patch)
        .output()
        .map_err(|e| Error::Xdelta(format!("no se pudo ejecutar xdelta3: {e}")))?;
    if !out.status.success() {
        return Err(Error::Xdelta("printhdrs rechazó el parche (¿no es un .xdelta?)".into()));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut total = 0u64;
    let mut windows = 0u32;
    for line in text.lines() {
        if let Some(rest) = line.trim().strip_prefix("VCDIFF target window length:") {
            total += rest.trim().parse::<u64>().map_err(|_| {
                Error::Xdelta("salida de printhdrs ilegible".into())
            })?;
            windows += 1;
        }
    }
    if windows == 0 {
        return Err(Error::Xdelta("el parche no declara ventanas VCDIFF".into()));
    }
    Ok(total)
}

/// Decodifica con `xdelta3 -d -n` (sin comprobar checksums, a propósito: ver
/// README — el wrapper CIA es único por dump y el contenido nuevo difiere).
/// Informa (bytes_escritos, total_esperado); si `cancel` se activa, mata el
/// proceso y limpia la salida parcial.
pub fn decode_forced(
    xdelta3: &Path,
    source: &Path,
    patch: &Path,
    dest: &Path,
    expected_total: u64,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(u64, u64),
) -> Result<()> {
    if dest.exists() {
        std::fs::remove_file(dest)?;
    }
    let mut child: Child = Command::new(xdelta3)
        .arg("-d")
        .arg("-n")
        .arg("-s")
        .arg(source)
        .arg(patch)
        .arg(dest)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| Error::Xdelta(format!("no se pudo lanzar xdelta3: {e}")))?;
    let start = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) {
            child.kill().ok();
            child.wait().ok();
            std::fs::remove_file(dest).ok();
            return Err(Error::Xdelta("cancelado por el usuario".into()));
        }
        match child.try_wait() {
            Err(e) => {
                child.kill().ok();
                return Err(Error::Xdelta(format!("error esperando a xdelta3: {e}")));
            }
            Ok(Some(status)) => {
                if !status.success() {
                    return Err(Error::Xdelta("xdelta3 falló decodificando".into()));
                }
                on_progress(expected_total, expected_total);
                break;
            }
            Ok(None) => {
                let n = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
                on_progress(n.min(expected_total), expected_total);
                if start.elapsed() > Duration::from_secs(3600) {
                    child.kill().ok();
                    return Err(Error::Xdelta("xdelta3 tardó más de 1h, abortado".into()));
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
    let final_len = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
    if final_len != expected_total {
        return Err(Error::Xdelta(format!(
            "salida incompleta: {final_len} de {expected_total} bytes"
        )));
    }
    Ok(())
}
