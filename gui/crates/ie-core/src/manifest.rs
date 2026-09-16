// SPDX-FileCopyrightText: 2026 Javiju
//
// SPDX-License-Identifier: MIT

//! Pack de traducción por ficheros: `manifiesto.json` + parches `.xdelta`.
//!
//! Formato (v57-interna, IE 1-2-3 ES): cada entrada declara la ruta del
//! fichero dentro del RomFS, su tamaño y SHA-256 original (puerta: la base
//! del usuario debe ser el mismo juego) y el tamaño y SHA-256 del resultado
//! (puerta: el parche aplicado bien). El parche se aplica con `decode_strict`.

use crate::error::{Error, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, serde::Deserialize)]
pub struct ManifestFile {
    /// Ruta dentro del RomFS, p. ej. `archive.fa`, `cro/ina_main1.cro`.
    /// Siempre con `/`, relativa, sin `..`.
    pub ruta: String,
    pub original_size: u64,
    pub original_sha256: String,
    pub resultado_size: u64,
    pub resultado_sha256: String,
    /// Ruta del `.xdelta` relativa a la carpeta del pack.
    pub parche: String,
    /// SHA-256 del propio fichero de parche (opcional en packs antiguos).
    #[serde(default)]
    pub parche_sha256: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct RawManifest {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub base: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub formato: String,
    pub ficheros: Vec<ManifestFile>,
}

#[derive(Debug, Clone)]
pub struct Manifest {
    pub version: String,
    pub base: String,
    pub dir: PathBuf,
    pub files: Vec<ManifestFile>,
}

impl Manifest {
    /// Ruta absoluta del parche n-ésimo dentro del pack.
    pub fn patch_path(&self, f: &ManifestFile) -> PathBuf {
        self.dir.join(&f.parche)
    }

    pub fn total_result_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.resultado_size).sum()
    }
}

fn is_hex64(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Carga y valida `dir/manifiesto.json` (sin tocar la base).
pub fn load(dir: &Path) -> Result<Manifest> {
    let path = dir.join("manifiesto.json");
    let text = std::fs::read_to_string(&path)
        .map_err(|_| Error::Format(format!("no se puede leer {}", path.display())))?;
    let raw: RawManifest = serde_json::from_str(&text)
        .map_err(|e| Error::Format(format!("manifiesto.json ilegible: {e}")))?;
    if raw.ficheros.is_empty() {
        return Err(Error::Format("el manifiesto no trae ficheros".into()));
    }
    for f in &raw.ficheros {
        let ruta = Path::new(&f.ruta);
        if ruta.is_absolute() || f.ruta.split('/').any(|c| c == ".." || c.is_empty()) {
            return Err(Error::Format(format!(
                "ruta rara en manifiesto: {}",
                f.ruta
            )));
        }
        if f.original_size == 0 || f.resultado_size == 0 {
            return Err(Error::Format(format!(
                "tamaño cero en manifiesto: {}",
                f.ruta
            )));
        }
        for (tag, v) in [
            ("original_sha256", &f.original_sha256),
            ("resultado_sha256", &f.resultado_sha256),
        ] {
            if !is_hex64(v.trim()) {
                return Err(Error::Format(format!("{tag} inválido en {}", f.ruta)));
            }
        }
        let pp = dir.join(&f.parche);
        if !pp.is_file() {
            return Err(Error::Format(format!("falta el parche {}", f.parche)));
        }
        if let Some(want) = &f.parche_sha256 {
            if !want.trim().is_empty() {
                let got = crate::normalize::sha256_file(
                    &pp,
                    &std::sync::atomic::AtomicBool::new(false),
                    &mut |_| {},
                )?;
                if got != want.trim().to_lowercase() {
                    return Err(Error::Verify(format!(
                        "el parche {} no cuadra con el manifiesto (¿descarga corrupta?)",
                        f.parche
                    )));
                }
            }
        }
    }
    Ok(Manifest {
        version: if raw.version.is_empty() {
            "(sin versión)".into()
        } else {
            raw.version
        },
        base: raw.base,
        dir: dir.to_path_buf(),
        files: raw.ficheros,
    })
}

/// Resumen de una línea para la tarjeta de la GUI (solo carpeta).
pub fn describe(dir: &Path) -> std::result::Result<String, String> {
    load(dir)
        .map(|m| {
            format!(
                "{} · {} ficheros{}",
                m.version,
                m.files.len(),
                if m.base.is_empty() {
                    String::new()
                } else {
                    format!(" · base: {}", elide(&m.base, 40))
                }
            )
        })
        .map_err(|e| e.to_string())
}

/// Resumen de una línea para la tarjeta de la GUI. Vale carpeta o `.zip`.
pub fn describe_pack(path: &Path) -> std::result::Result<String, String> {
    if path.is_dir() {
        return describe(path);
    }
    let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(f).map_err(|_| "no es un .zip válido".to_string())?;
    let mut mf = zip
        .by_name("manifiesto.json")
        .map_err(|_| "el .zip no trae manifiesto.json en la raíz".to_string())?;
    let mut text = String::new();
    use std::io::Read as _;
    mf.read_to_string(&mut text).map_err(|e| e.to_string())?;
    let raw: RawManifest =
        serde_json::from_str(&text).map_err(|e| format!("manifiesto.json ilegible: {e}"))?;
    Ok(format!(
        "{} · {} ficheros (.zip){}",
        if raw.version.is_empty() { "(sin versión)" } else { &raw.version },
        raw.ficheros.len(),
        if raw.base.is_empty() { String::new() } else { format!(" · base: {}", elide(&raw.base, 40)) }
    ))
}

/// Si es carpeta la devuelve tal cual; si es `.zip` lo extrae a
/// `work/pack` (limpio antes) y devuelve eso.
pub fn prepare_pack(
    pack: &Path,
    work: &Path,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<PathBuf> {
    use std::sync::atomic::Ordering;
    if pack.is_dir() {
        return Ok(pack.to_path_buf());
    }
    let dest = work.join("pack");
    if dest.exists() {
        std::fs::remove_dir_all(&dest)?;
    }
    std::fs::create_dir_all(&dest)?;
    let f = std::fs::File::open(pack)?;
    let mut zip = zip::ZipArchive::new(f).map_err(|e| Error::Zip(format!("no es un .zip válido: {e}")))?;
    for i in 0..zip.len() {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let mut e = zip.by_index(i).map_err(|e| Error::Zip(format!("{e}")))?;
        let name = e.name().to_owned();
        // Sin zip-slip: solo rutas relativas limpias.
        if name.starts_with('/') || name.starts_with('\\') || name.split('/').any(|c| c == "..") {
            return Err(Error::Zip(format!("ruta rara en el zip: {name}")));
        }
        let out = dest.join(&name);
        if e.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(p) = out.parent() {
            std::fs::create_dir_all(p)?;
        }
        let mut o = std::fs::File::create(&out)?;
        {
            use std::io::{Read as _, Write as _};
            let mut buf = vec![0u8; 8 * 1024 * 1024];
            loop {
                let n = e.read(&mut buf).map_err(|e| Error::Zip(format!("{e}")))?;
                if n == 0 {
                    break;
                }
                o.write_all(&buf[..n])?;
            }
            o.flush()?;
        }
    }
    Ok(dest)
}

fn elide(s: &str, max: usize) -> String {
    let c: Vec<char> = s.chars().collect();
    if c.len() <= max {
        return s.to_owned();
    }
    format!("{}...", c[..max - 3].iter().collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack_ok(dir: &Path) {
        std::fs::create_dir_all(dir.join("parches")).unwrap();
        std::fs::write(dir.join("parches/a.xdelta"), b"XD").unwrap();
        std::fs::write(
            dir.join("manifiesto.json"),
            r#"{"version":"t","base":"b","ficheros":[
                {"ruta":"a/fa","original_size":3,"original_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","resultado_size":4,"resultado_sha256":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","parche":"parches/a.xdelta"}]}"#,
        )
        .unwrap();
    }

    #[test]
    fn manifiesto_valido_y_resumen() {
        let dir = std::env::temp_dir().join(format!("ie-man-{}.d", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        pack_ok(&dir);
        let m = load(&dir).unwrap();
        assert_eq!(m.version, "t");
        assert_eq!(m.files.len(), 1);
        assert!(describe(&dir).unwrap().contains("1 ficheros"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn zip_se_prepara_y_describe() {
        use std::io::Write as _;
        let dir = std::env::temp_dir().join(format!("ie-manzip-{}.d", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        pack_ok(&dir);
        // empaqueta el pack a .zip en memoria
        let mut zw = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for rel in ["manifiesto.json", "parches/a.xdelta"] {
            zw.start_file(rel, zip::write::SimpleFileOptions::default()).unwrap();
            zw.write_all(&std::fs::read(dir.join(rel)).unwrap()).unwrap();
        }
        let buf = zw.finish().unwrap().into_inner();
        let zp = dir.join("pack.zip");
        std::fs::write(&zp, &buf).unwrap();
        let d = describe_pack(&zp).unwrap();
        assert!(d.contains("1 ficheros") && d.contains("(.zip)"), "{d}");
        let work = dir.join("work");
        let out = prepare_pack(&zp, &work, &std::sync::atomic::AtomicBool::new(false)).unwrap();
        assert_eq!(load(&out).unwrap().files.len(), 1);
        // la carpeta pasa tal cual
        assert_eq!(
            prepare_pack(&dir, &work, &std::sync::atomic::AtomicBool::new(false)).unwrap(),
            dir
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rechaza_rutas_con_trampas() {
        let dir = std::env::temp_dir().join(format!("ie-man-bad-{}.d", std::process::id()));
        std::fs::create_dir_all(dir.join("parches")).unwrap();
        std::fs::write(dir.join("parches/a.xdelta"), b"XD").unwrap();
        std::fs::write(
            dir.join("manifiesto.json"),
            r#"{"ficheros":[
                {"ruta":"../fuera","original_size":1,"original_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","resultado_size":1,"resultado_sha256":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","parche":"parches/a.xdelta"}]}"#,
        )
        .unwrap();
        assert!(load(&dir).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
