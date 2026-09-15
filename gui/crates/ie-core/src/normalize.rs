// SPDX-FileCopyrightText: 2026 Javiju
//
// SPDX-License-Identifier: MIT

//! Normalización de la base a CCI descifrado canónico + utilidades compartidas.
//!
//! Para parches distribuidos sobre `.3ds` descifrado (p. ej. IE 1-2-3), donde
//! el modo forzado NO vale: si la base no es byte-idéntica, el resultado
//! sería una quimera. Aquí la base se normaliza (descifra) y el parche se
//! aplica en modo estricto, con checksums y hash final como guardianes.

use crate::error::{Error, Result};
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    Cia,
    Cci,
}

/// Detecta CIA por cabecera, CCI por magia NCSD y rechaza CXI suelto.
pub fn detect(path: &Path) -> Result<InputKind> {
    let mut f = std::fs::File::open(path)?;
    let mut head = [0u8; 0x204];
    let n = f.read(&mut head)?;
    if n < 0x204 {
        return Err(Error::Format("archivo demasiado pequeño".into()));
    }
    if u32::from_le_bytes(head[0..4].try_into().unwrap()) == 0x2020 {
        return Ok(InputKind::Cia);
    }
    match &head[0x100..0x104] {
        b"NCSD" => Ok(InputKind::Cci),
        b"NCCH" => Err(Error::Format(
            "es un CXI suelto: se necesita el CIA o el CCI (.3ds) completo".into(),
        )),
        _ => Err(Error::Format("no es un CIA ni un CCI de 3DS".into())),
    }
}

/// SHA-256 en streaming (puertas de hash de los perfiles).
pub fn sha256_file(path: &Path, cancel: &AtomicBool, mut on_chunk: impl FnMut(u64)) -> Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        on_chunk(n as u64);
    }
    Ok(format!("{:x}", h.finalize()))
}

/// SHA-256 del CCI excluyendo la cabecera NCSD ([0..0x200]).
///
/// La cabecera depende del linaje del dump (firma RSA real del cartucho vs
/// relleno de makerom/nuestro, flags de tarjeta) y NUNCA puede coincidir
/// entre una base cartucho y una convertida desde CIA — aunque el juego sea
/// byte-idéntico. El contenido (particiones) sí es comparable: es lo que
/// verifica esta puerta. Ver NOTA_IE123.md.
pub fn sha256_cci_content(path: &Path, cancel: &AtomicBool, mut on_chunk: impl FnMut(u64)) -> Result<String> {
    use std::io::SeekFrom;
    let mut f = std::fs::File::open(path)?;
    let total = f.seek(SeekFrom::End(0))?;
    if total <= 0x200 {
        return Err(Error::Format("CCI demasiado pequeño".into()));
    }
    f.seek(SeekFrom::Start(0x200))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024 * 1024];
    let mut left = total - 0x200;
    while left > 0 {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let n = left.min(buf.len() as u64) as usize;
        f.read_exact(&mut buf[..n])?;
        h.update(&buf[..n]);
        left -= n as u64;
        on_chunk(n as u64);
    }
    Ok(format!("{:x}", h.finalize()))
}

fn copy_stream(src: &Path, dst: &Path, cancel: &AtomicBool, mut on_chunk: impl FnMut(u64)) -> Result<u64> {
    let mut f = std::fs::File::open(src)?;
    let mut o = std::fs::File::create(dst)?;
    let mut buf = vec![0u8; 64 * 1024 * 1024];
    let mut done = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        o.write_all(&buf[..n])?;
        done += n as u64;
        on_chunk(n as u64);
    }
    o.flush()?;
    Ok(done)
}

fn read_ncch_at(path: &Path, off: u64) -> Result<crate::ncch::NcchHeader> {
    let mut f = std::fs::File::open(path)?;
    f.seek(SeekFrom::Start(off))?;
    let mut raw = [0u8; 0x200];
    f.read_exact(&mut raw)?;
    crate::ncch::NcchHeader::parse(raw)
}

pub struct NormalizedCci {
    /// CCI descifrado en temporal (layout canónico para el parche).
    pub path: PathBuf,
    pub title_id: u64,
    pub product: String,
    pub was_encrypted: bool,
}

/// Normaliza la base (CIA o CCI) a CCI descifrado canónico en `work`:
///
/// - CIA: copia, descifra el content0 in place y construye el CCI temporal.
/// - CCI: copia y descifra el content0 (partición 0) in place.
/// - Si ya estaba descifrado, solo copia (y avisa con `was_encrypted=false`).
pub fn normalize_to_decrypted_cci(
    input: &Path,
    work: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<NormalizedCci> {
    match detect(input)? {
        InputKind::Cia => {
            let layout = crate::cia::read_layout(input)?;
            let total = layout.total_size;
            if layout.contents.is_empty() {
                return Err(Error::Format("el CIA no trae contenidos".into()));
            }
            let mut idx: Vec<u16> = layout.contents.iter().map(|c| c.index).collect();
            idx.sort_unstable();
            let first = layout.content(idx[0])?.clone();
            let orig = read_ncch_at(input, first.offset)?;
            let was_encrypted = !orig.is_decrypted();
            let tmp_cia = work.join("norm_base.cia");
            // Copia: 0..400. Descifrado: 400..900 (repartido entre
            // contenidos). CCI temporal: 900..1000.
            let mut done = 0u64;
            copy_stream(input, &tmp_cia, cancel, &mut |d| {
                done += d;
                progress(done * 400 / total.max(1), 1000);
            })?;
            let n = idx.len() as u64;
            let mut hdr = None;
            for (k, i) in idx.iter().enumerate() {
                let c = layout.content(*i)?.clone();
                // Solo se descifra lo descifrable: sin ExeFS (p. ej. manuales
                // CFA) no hay nada que nuestro descifrador maneje y se deja
                // tal cual — precedente Galaxy: el manual viaja cifrado en
                // el CCI final y Azahar arranca igual.
                let h0 = read_ncch_at(&tmp_cia, c.offset)?;
                if h0.is_decrypted() || h0.exefs_size < 0x200 {
                    if hdr.is_none() {
                        hdr = Some(h0);
                    }
                    continue;
                }
                let h = crate::ncch::decrypt_content_in_place(&tmp_cia, c.offset, cancel, &mut |d, t| {
                    progress(400 + (k as u64 * 500 + d * 500 / t.max(1)) / n.max(1), 1000);
                })?;
                if hdr.is_none() {
                    hdr = Some(h);
                }
            }
            let hdr = hdr.ok_or_else(|| Error::Format("sin contenidos descifrados".into()))?;
            let out = work.join("norm_base.3ds");
            let parts: Vec<crate::cci::CciPart> = idx
                .iter()
                .map(|i| {
                    let c = layout.content(*i).unwrap().clone();
                    crate::cci::CciPart { path: &tmp_cia, offset: c.offset, len: c.size }
                })
                .collect();
            let grand: u64 = parts.iter().map(|p| p.len).sum();
            crate::cci::build_cci_packed(&parts, hdr.title_id(), &out, cancel, &mut |d, _| {
                progress(900 + d * 100 / grand.max(1), 1000);
            })?;
            progress(1000, 1000);
            Ok(NormalizedCci {
                path: out,
                title_id: hdr.title_id(),
                product: hdr.product_code(),
                was_encrypted,
            })
        }
        InputKind::Cci => {
            let total = std::fs::metadata(input)?.len();
            let tmp = work.join("norm_base.3ds");
            let mut done = 0u64;
            copy_stream(input, &tmp, cancel, &mut |d| {
                done += d;
                progress(done * 500 / total.max(1), 1000);
            })?;
            let parts = crate::cci::partitions(&tmp)?;
            let (p0_off, _p0_len) = *parts.first().ok_or_else(|| Error::Format("CCI sin particiones".into()))?;
            let was_encrypted = !read_ncch_at(&tmp, p0_off)?.is_decrypted();
            let hdr = crate::ncch::decrypt_content_in_place(&tmp, p0_off, cancel, &mut |d, t| {
                progress(500 + d * 500 / t.max(1), 1000);
            })?;
            progress(1000, 1000);
            Ok(NormalizedCci {
                path: tmp,
                title_id: hdr.title_id(),
                product: hdr.product_code(),
                was_encrypted,
            })
        }
    }
}
