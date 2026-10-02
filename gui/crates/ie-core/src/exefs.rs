// SPDX-FileCopyrightText: 2026 Javiju
//
// SPDX-License-Identifier: MIT

//! Parcheo mínimo del ExeFS para el manifiesto (`banner`, `icon`).
//!
//! Layout verificado contra builds reales (IE 1-2-3):
//! - Cabecera de 0x200: 10 entradas de 16 B (`nombre[8]`, `offset u32`,
//!   `size u32`) en `[0, 0xA0)`; los datos empiezan en `+0x200` con
//!   `offset` relativo a esa base (ficheros alineados a 0x200).
//! - Los SHA-256 por fichero viven en `[0xA0, 0x200)` pero NO en el orden
//!   de las entradas (observado: code->0x1E0, banner->0x1C0, icon->0x1A0,
//!   logo->0x180). Para no depender del orden, el slot se localiza
//!   buscando el SHA actual de los datos (debe aparecer exactamente 1 vez).
//! - Solo mismo tamaño (banner e icono son contenedores fijos): así no se
//!   mueve el layout y basta rehashear fichero + superblock (`0x1C0`).

use crate::error::{Error, Result};
use sha2::{Digest, Sha256};

pub const HEADER_LEN: usize = 0x200;
pub const DATA_BASE: u64 = 0x200;
/// Lo único parcheable en v1. El `.code` queda prohibido a propósito.
pub const ALLOWED: [&str; 2] = ["banner", "icon"];

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub offset: u32,
    pub size: u32,
}

fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// Lee el directorio del ExeFS (cabecera de 0x200). Omite entradas vacías.
pub fn read_dir(exefs: &[u8]) -> Result<Vec<Entry>> {
    if exefs.len() < HEADER_LEN {
        return Err(Error::Format("ExeFS menor que su cabecera".into()));
    }
    let mut out = Vec::new();
    for i in 0..10 {
        let o = i * 16;
        let raw = &exefs[o..o + 8];
        let end = raw.iter().position(|&c| c == 0).unwrap_or(8);
        if end == 0 {
            continue;
        }
        let name = String::from_utf8_lossy(&raw[..end]).into_owned();
        let (offset, size) = (u32le(exefs, o + 8), u32le(exefs, o + 12));
        let end64 = DATA_BASE + offset as u64 + size as u64;
        if end64 > exefs.len() as u64 {
            return Err(Error::Format(format!("entrada ExeFS fuera de rango: {name}")));
        }
        out.push(Entry { name, offset, size });
    }
    if out.is_empty() {
        return Err(Error::Format("ExeFS sin entradas".into()));
    }
    Ok(out)
}

/// `banner.bnr` -> `banner`, `icon.bin` -> `icon`. Solo nombres planos de
/// la lista permitida; `None` en cualquier otro caso (rutas, `.code`...).
pub fn normalize_name(ruta: &str) -> Option<String> {
    if ruta.contains('/') || ruta.contains('\\') || ruta.contains("..") {
        return None;
    }
    let base = ruta.rsplit_once('.').map(|(s, _)| s).unwrap_or(ruta);
    if base.is_empty() || base.len() > 8 {
        return None;
    }
    if ALLOWED.contains(&base) {
        Some(base.to_owned())
    } else {
        None
    }
}

/// Rango absoluto de los datos del fichero dentro del blob ExeFS.
pub fn data_range(exefs_len: u64, e: &Entry) -> (u64, u64) {
    let s = DATA_BASE + e.offset as u64;
    let _ = exefs_len;
    (s, s + e.size as u64)
}

/// Localiza el slot del hash (SHA actual de los datos) en `[0xA0, 0x200)`.
fn locate_hash_slot(header: &[u8], data: &[u8]) -> Result<usize> {
    let want = Sha256::digest(data);
    let mut found = None;
    let mut n = 0;
    for slot in 0..((HEADER_LEN - 0xA0) / 32) {
        let o = 0xA0 + slot * 32;
        if header[o..o + 32] == want[..] {
            found = Some(o);
            n += 1;
        }
    }
    match (found, n) {
        (Some(o), 1) => Ok(o),
        (_, 0) => Err(Error::Format("hash ExeFS original no localizado (¿base distinta?)".into())),
        _ => Err(Error::Format("hash ExeFS ambiguo (duplicado, pack no soportado)".into())),
    }
}

/// Empalma `new_data` (mismo tamaño exacto) y actualiza su hash.
/// Solo nombres de la lista permitida (nunca `.code`).
/// Devuelve el SHA nuevo para verificación.
pub fn apply(exefs: &mut [u8], name: &str, new_data: &[u8]) -> Result<[u8; 32]> {
    if !ALLOWED.contains(&name) {
        return Err(Error::Format(format!("{name}: no parcheable en v1 (solo banner/icon)")));
    }
    let dir = read_dir(exefs)?;
    let e = dir
        .iter()
        .find(|e| e.name == name)
        .ok_or_else(|| Error::Format(format!("el ExeFS base no trae {name}")))?;
    if new_data.len() as u64 != e.size as u64 {
        return Err(Error::Format(format!(
            "{name}: tamaño distinto ({} vs {}), en v1 debe ser igual",
            new_data.len(),
            e.size
        )));
    }
    let (s, en) = data_range(exefs.len() as u64, e);
    let slot = locate_hash_slot(&exefs[..HEADER_LEN], &exefs[s as usize..en as usize])?;
    exefs[s as usize..en as usize].copy_from_slice(new_data);
    let new_hash: [u8; 32] = Sha256::digest(new_data).into();
    exefs[slot..slot + 32].copy_from_slice(&new_hash);
    Ok(new_hash)
}

/// Hash del superblock para el NCCH `0x1C0`: SHA-256 de los primeros
/// `hash_len` bytes del ExeFS.
pub fn superblock_hash(exefs: &[u8], hash_len: u64) -> Result<[u8; 32]> {
    let n = hash_len as usize;
    if n == 0 || n > exefs.len() {
        return Err(Error::Format("cobertura de hash ExeFS incoherente".into()));
    }
    Ok(Sha256::digest(&exefs[..n]).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Blob sintético: code/banner/icon con hashes a propósito
    /// DESORDENADOS (como en roms reales) para probar la localización.
    fn synth() -> Vec<u8> {
        let files: [(&[u8], Vec<u8>); 3] = [
            (b".code\0\0\0", vec![0x11u8; 0x300]),
            (b"banner\0\0", vec![0x22u8; 0x200]),
            (b"icon\0\0\0\0", vec![0x33u8; 0x100]),
        ];
        // offsets relativos a +0x200, alineados a 0x200 como makerom
        let mut off = 0u32;
        let mut offs = Vec::new();
        for (_, d) in &files {
            offs.push(off);
            off += (d.len() as u32).next_multiple_of(0x200);
        }
        let total = 0x200 + off as usize;
        let mut b = vec![0u8; total];
        for (i, (nm, d)) in files.iter().enumerate() {
            b[i * 16..i * 16 + 8].copy_from_slice(nm);
            b[i * 16 + 8..i * 16 + 12].copy_from_slice(&offs[i].to_le_bytes());
            b[i * 16 + 12..i * 16 + 16].copy_from_slice(&(d.len() as u32).to_le_bytes());
            let s = 0x200 + offs[i] as usize;
            b[s..s + d.len()].copy_from_slice(d);
        }
        // hashes desordenados: code->slot 10, banner->slot 9, icon->slot 8
        let hashes = [
            Sha256::digest(files[0].1.as_slice()),
            Sha256::digest(files[1].1.as_slice()),
            Sha256::digest(files[2].1.as_slice()),
        ];
        b[0x1E0..0x200].copy_from_slice(&hashes[0]);
        b[0x1C0..0x1E0].copy_from_slice(&hashes[1]);
        b[0x1A0..0x1C0].copy_from_slice(&hashes[2]);
        b
    }

    #[test]
    fn lee_directorio_y_aplica_banner() {
        let mut b = synth();
        let dir = read_dir(&b).unwrap();
        assert_eq!(dir.len(), 3);
        assert_eq!(dir[1].name, "banner");
        let nuevo = vec![0x99u8; 0x200];
        let h = apply(&mut b, "banner", &nuevo).unwrap();
        let want: [u8; 32] = Sha256::digest(&nuevo).into();
        assert_eq!(h, want);
        // empalmado + hash actualizado, resto intacto
        assert_eq!(&b[0x200 + 0x400..0x200 + 0x600], &nuevo[..]);
        assert_eq!(&b[0x200..0x200 + 0x300], &vec![0x11u8; 0x300][..]);
        assert_eq!(&b[0x1C0..0x1E0], &h[..]);
        // superblock cambia en consecuencia
        let sb = superblock_hash(&b, 0x200).unwrap();
        let want_sb: [u8; 32] = Sha256::digest(&b[..0x200]).into();
        assert_eq!(sb, want_sb);
    }

    #[test]
    fn rechaza_tamano_distinto_y_code() {
        let mut b = synth();
        assert!(apply(&mut b, "banner", &vec![0u8; 0x201]).is_err());
        assert!(apply(&mut b, ".code", &vec![0u8; 0x300]).is_err());
        assert!(apply(&mut b, "logo", &[0u8; 4]).is_err());
    }

    #[test]
    fn nombres_planos_y_permitidos() {
        assert_eq!(normalize_name("banner.bnr").as_deref(), Some("banner"));
        assert_eq!(normalize_name("icon.bin").as_deref(), Some("icon"));
        assert_eq!(normalize_name("banner").as_deref(), Some("banner"));
        assert!(normalize_name(".code").is_none());
        assert!(normalize_name("a/b").is_none());
        assert!(normalize_name("../x").is_none());
        assert!(normalize_name("banner.bnr.xdelta").is_none());
    }
}
