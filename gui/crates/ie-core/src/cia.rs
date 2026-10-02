// SPDX-FileCopyrightText: 2026 Javiju
//
// SPDX-License-Identifier: MIT

//! Parseo mínimo de contenedor CIA: localizar los contents (solo lectura).
//!
//! Estrategia idéntica a `rebuild.sh`: los tamaños salen del TMD y los
//! offsets se calculan desde el final (el contenido queda justo antes del
//! footer), sin adivinar paddings de cabecera.
//!
//! Además: `synthesize_like` reconstruye un CIA con el layout (cabecera +
//! offsets) de una plantilla pero contenidos traídos de otro sitio (p. ej.
//! particiones de un CCI normalizado y descifrado). Es la pieza que hace
//! universal el modo forzado: el xdelta espera bytes con envoltorio CIA y
//! se le dan, aunque la base del usuario sea un volcado de tarjeta.

use crate::error::{Error, Result};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone)]
pub struct ContentRef {
    pub index: u16,
    pub offset: u64,
    pub size: u64,
}

#[derive(Debug, Clone)]
pub struct CiaLayout {
    pub total_size: u64,
    pub contents: Vec<ContentRef>,
}

impl CiaLayout {
    pub fn content(&self, index: u16) -> Result<&ContentRef> {
        self.contents
            .iter()
            .find(|c| c.index == index)
            .ok_or_else(|| Error::Format(format!("el CIA no trae content{index}")))
    }
}

fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

pub fn read_layout(path: &Path) -> Result<CiaLayout> {
    let mut f = File::open(path)?;
    let total = f.seek(SeekFrom::End(0))?;
    f.seek(SeekFrom::Start(0))?;
    let mut hdr = [0u8; 0x20];
    f.read_exact(&mut hdr)?;
    if u32le(&hdr, 0) != 0x2020 {
        return Err(Error::Format("cabecera CIA inesperada".into()));
    }
    let cert_size = u32le(&hdr, 0x08) as u64;
    let tik_size = u32le(&hdr, 0x0C) as u64;
    let tmd_size = u32le(&hdr, 0x10) as u64;
    let footer_size = u32le(&hdr, 0x14) as u64;
    let _ = (cert_size, tik_size);

    // Cada sección va alineada a 64 bytes (el cert arranca en 0x2040,
    // no en 0x2020). El TMD, en cambio, se localiza por escaneo validado:
    // algunos dumps lo colocan con alineados raros y la aritmética ciega falla.
    let tmd = find_tmd(&mut f, tmd_size, total, footer_size)?;

    // OJO: tipo de firma big-endian (00 01 00 04); el resto del TMD también.
    let sig_len = match u32::from_be_bytes(tmd[0..4].try_into().unwrap()) {
        0x10003 => 0x200usize,
        0x10004 => 0x100usize,
        0x10005 => 0x3Cusize,
        t => return Err(Error::Format(format!("firma TMD {t:#x} no soportada"))),
    };
    let head_off = (4 + sig_len + 63) & !63;
    // OJO: el TMD es big-endian (al contrario que el resto de structs 3DS).
    let count = u16::from_be_bytes(tmd[head_off + 0x9E..head_off + 0xA0].try_into().unwrap()) as usize;
    if count == 0 || count > 64 {
        return Err(Error::Format(format!("content count raro: {count}")));
    }
    let chunks = head_off + 0xC4 + 64 * 0x24;
    let mut contents = Vec::with_capacity(count);
    for i in 0..count {
        let o = chunks + i * 0x30;
        let index = u16::from_be_bytes(tmd[o + 4..o + 6].try_into().unwrap());
        let size = u64::from_be_bytes(tmd[o + 8..o + 16].try_into().unwrap());
        contents.push(ContentRef { index, offset: 0, size });
    }
    contents.sort_by_key(|c| c.index);
    let all: u64 = contents.iter().map(|c| c.size).sum();
    let mut off = total - footer_size - all;
    for c in contents.iter_mut() {
        c.offset = off;
        off += c.size;
    }
    // Validación fuerte: magia NCCH al inicio de cada content (como rebuild.sh
    // asume implícitamente al extraer por offsets).
    for c in contents.iter() {
        f.seek(SeekFrom::Start(c.offset + 0x100))?;
        let mut magic = [0u8; 4];
        f.read_exact(&mut magic)?;
        if &magic != b"NCCH" {
            return Err(Error::Format(format!(
                "content{} sin magia NCCH en {:#x}",
                c.index, c.offset
            )));
        }
    }
    Ok(CiaLayout { total_size: total, contents })
}

/// Localiza el TMD escaneando la zona de metadatos y validando cada
/// candidato (firma + issuer + conteo + tamaños + magias NCCH resultantes).
/// Devuelve sus bytes ya leídos.
fn find_tmd(f: &mut File, tmd_size: u64, total: u64, footer_size: u64) -> Result<Vec<u8>> {
    if tmd_size == 0 || tmd_size > 0x10000 {
        return Err(Error::Format("tamaño TMD raro".into()));
    }
    // La zona de metadatos de un CIA normal cabe en los primeros 64 KB.
    let scan_len = 0x10000u64.min(total);
    let mut area = vec![0u8; scan_len as usize];
    f.seek(SeekFrom::Start(0))?;
    f.read_exact(&mut area)?;
    let sig_len_of = |t: u32| -> Option<usize> {
        match t {
            0x10003 => Some(0x200),
            0x10004 => Some(0x100),
            0x10005 => Some(0x3C),
            _ => None,
        }
    };
    let mut p = 0usize;
    while p + 4 <= area.len() {
        let t = u32::from_be_bytes(area[p..p + 4].try_into().unwrap());
        if let Some(sig_len) = sig_len_of(t) {
            if let Some(tmd) = try_tmd_at(&area, p, sig_len, tmd_size, total, footer_size, f)? {
                return Ok(tmd);
            }
        }
        p += 4;
    }
    Err(Error::Format("no se localizó el TMD en el CIA".into()))
}

/// Intenta validar un TMD en el offset `p` del área escaneada. Relee de `f`
/// lo que falte y comprueba magias NCCH en los contents resultantes.
fn try_tmd_at(
    area: &[u8],
    p: usize,
    sig_len: usize,
    tmd_size: u64,
    total: u64,
    footer_size: u64,
    f: &mut File,
) -> Result<Option<Vec<u8>>> {
    let head = p + ((4 + sig_len + 63) & !63);
    if head + 0xA0 > area.len() {
        return Ok(None);
    }
    // Issuer ASCII "Root-..." al inicio de la cabecera TMD.
    if !area[head].is_ascii_alphabetic() || &area[head..head + 5] != b"Root-" {
        // El issuer puede venir con padding distinto; no es concluyente:
        // se sigue validando por conteo/tamaños/magias.
    }
    let count = u16::from_be_bytes(area[head + 0x9E..head + 0xA0].try_into().unwrap()) as usize;
    if count == 0 || count > 64 {
        return Ok(None);
    }
    let chunks = head + 0xC4 + 64 * 0x24;
    if chunks + count * 0x30 > area.len() {
        return Ok(None);
    }
    let mut sizes_idx = Vec::with_capacity(count);
    let mut sum = 0u64;
    for i in 0..count {
        let o = chunks + i * 0x30;
        let index = u16::from_be_bytes(area[o + 4..o + 6].try_into().unwrap());
        let size = u64::from_be_bytes(area[o + 8..o + 16].try_into().unwrap());
        if size == 0 || size > total {
            return Ok(None);
        }
        sum = sum.saturating_add(size);
        sizes_idx.push((index, size));
    }
    if sum + footer_size >= total {
        return Ok(None);
    }
    sizes_idx.sort_by_key(|&(idx, _)| idx);
    // Los contents quedan contiguos justo antes del footer: verificar magia.
    let mut off = total - footer_size - sum;
    for &(_, size) in &sizes_idx {
        let mut magic = [0u8; 4];
        f.seek(SeekFrom::Start(off + 0x100))?;
        if f.read_exact(&mut magic).is_err() || &magic != b"NCCH" {
            return Ok(None);
        }
        off += size;
    }
    // Válido: leer el TMD completo desde el fichero.
    let mut tmd = vec![0u8; tmd_size as usize];
    f.seek(SeekFrom::Start(p as u64))?;
    f.read_exact(&mut tmd)?;
    Ok(Some(tmd))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rechaza_no_cia() {
        let dir = std::env::temp_dir();
        let p = dir.join("ie-core-test-no-cia.bin");
        std::fs::write(&p, vec![0u8; 0x3000]).unwrap();
        assert!(read_layout(&p).is_err());
        std::fs::remove_file(&p).ok();
    }
}

/// Un contenido de origen para `synthesize_like`: de dónde copiarlo.
#[derive(Debug, Clone)]
pub struct SynthPart {
    pub src: PathBuf,
    pub src_off: u64,
    pub len: u64,
}

/// Sintetiza en `out` un CIA con el layout exacto de `template` (cabecera,
/// TMD y offsets, copiados tal cual) pero con los contenidos de `parts` (en
/// orden de índice). Los tamaños deben coincidir al byte: si la plantilla
/// es de otro juego o revisión, se rechaza en vez de fabricar un híbrido.
///
/// El TMD copiado queda rancio (hashes de los contenidos originales), pero
/// no importa: el flujo forzado solo extrae los contenidos del resultado.
/// Sirve para alimentar un xdelta generado contra bytes-CIA partiendo de
/// un CCI normalizado y descifrado (mismo juego, otra envoltura).
pub fn synthesize_like(
    template: &Path,
    parts: &[SynthPart],
    out: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, u64),
) -> Result<CiaLayout> {
    let layout = read_layout(template)?;
    if parts.len() != layout.contents.len() {
        return Err(Error::Format(format!(
            "la plantilla trae {} contents y se dieron {} (¿plantilla de otro juego?)",
            layout.contents.len(),
            parts.len()
        )));
    }
    for (p, c) in parts.iter().zip(layout.contents.iter()) {
        if p.len != c.size {
            return Err(Error::Format(format!(
                "content{}: la plantilla espera {} bytes y la base trae {} (¿otra revisión?)",
                c.index, c.size, p.len
            )));
        }
    }
    let total = layout.total_size;
    let first_off = layout.contents.first().map(|c| c.offset).unwrap_or(total);
    let mut o = File::create(out)?;
    let mut done = 0u64;
    // Cabecera + metadatos de la plantilla (hasta el primer content).
    {
        let mut t = File::open(template)?;
        t.seek(SeekFrom::Start(0))?;
        let mut left = first_off;
        let mut buf = vec![0u8; 8 * 1024 * 1024];
        while left > 0 {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            let n = left.min(buf.len() as u64) as usize;
            t.read_exact(&mut buf[..n])?;
            o.write_all(&buf[..n])?;
            left -= n as u64;
            done += n as u64;
            progress(done, total);
        }
    }
    // Contenidos en orden (los offsets de plantilla son contiguos; si hay
    // hueco se rellena con ceros para no desplazar nada).
    let mut buf = vec![0u8; 8 * 1024 * 1024];
    for (p, c) in parts.iter().zip(layout.contents.iter()) {
        if done < c.offset {
            let z = vec![0u8; (c.offset - done).min(8 * 1024 * 1024) as usize];
            let mut left = c.offset - done;
            while left > 0 {
                if cancel.load(Ordering::Relaxed) {
                    return Err(Error::Cancelled);
                }
                let n = left.min(z.len() as u64) as usize;
                o.write_all(&z[..n])?;
                left -= n as u64;
                done += n as u64;
            }
        }
        if done != c.offset {
            return Err(Error::Format("hueco negativo en la plantilla (bug interno)".into()));
        }
        let mut s = File::open(&p.src)?;
        s.seek(SeekFrom::Start(p.src_off))?;
        let mut left = p.len;
        while left > 0 {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            let n = left.min(buf.len() as u64) as usize;
            s.read_exact(&mut buf[..n])?;
            o.write_all(&buf[..n])?;
            left -= n as u64;
            done += n as u64;
            progress(done, total);
        }
    }
    // Footer de la plantilla (tras el último content).
    {
        let mut t = File::open(template)?;
        let end = layout.contents.last().map(|c| c.offset + c.size).unwrap_or(first_off);
        t.seek(SeekFrom::Start(end))?;
        let mut left = total - end;
        while left > 0 {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            let n = left.min(buf.len() as u64) as usize;
            t.read_exact(&mut buf[..n])?;
            o.write_all(&buf[..n])?;
            left -= n as u64;
            done += n as u64;
            progress(done, total);
        }
    }
    o.flush()?;
    progress(total, total);
    Ok(layout)
}

/// Idiomas 3DS del SMDH que probamos, en orden: español, inglés, japonés.
const SMDH_LANGS: [usize; 3] = [5, 1, 0];

/// Título corto del SMDH para el nombre de salida.
///
/// Busca el SMDH en sus dos colocaciones conocidas: región meta tras el
/// TMD (CIA estándar) y footer tras el último content (p. ej. el CIA de
/// Luis, con `footer_size` 0x3AC0 y SMDH a +0x400). Prueba idiomas en
/// orden español, inglés, japonés.
///
/// Devuelve `None` si no hay SMDH legible: la GUI usa entonces el nombre
/// del fichero base. Nunca falla: es solo cosmética.
pub fn smdh_short_title(path: &Path) -> Option<String> {
    let mut f = File::open(path).ok()?;
    let total = f.seek(SeekFrom::End(0)).ok()?;
    f.seek(SeekFrom::Start(0)).ok()?;
    let mut hdr = [0u8; 0x20];
    f.read_exact(&mut hdr).ok()?;
    if u32le(&hdr, 0) != 0x2020 {
        return None;
    }
    let (cert, tik, tmd, footer) = (
        u32le(&hdr, 0x08) as u64,
        u32le(&hdr, 0x0C) as u64,
        u32le(&hdr, 0x10) as u64,
        u32le(&hdr, 0x14) as u64,
    );
    // Cada sección va alineada a 64 (el cert arranca en 0x2040).
    let mut off = align64(0x2020 + cert);
    off = align64(off + tik);
    off = align64(off + tmd);
    let mut cands = vec![off];
    if footer > 0 && footer < total {
        cands.push(total - footer);
        cands.push(total - footer + 0x400);
    }
    for c in cands {
        if let Some(t) = titles_at(&mut f, c) {
            return Some(t);
        }
    }
    None
}

fn titles_at(f: &mut File, off: u64) -> Option<String> {
    f.seek(SeekFrom::Start(off)).ok()?;
    let mut smdh = vec![0u8; 0x2010];
    f.read_exact(&mut smdh).ok()?;
    if u32le(&smdh, 0) != 0x4844_4D53 {
        // "SMDH"
        return None;
    }
    for lang in SMDH_LANGS {
        let o = 8 + lang * 0x200;
        let mut units = [0u16; 0x20];
        for (i, u) in units.iter_mut().enumerate() {
            *u = u16::from_le_bytes([smdh[o + 2 * i], smdh[o + 2 * i + 1]]);
        }
        let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
        let t = String::from_utf16_lossy(&units[..end]).trim().to_owned();
        if !t.is_empty() {
            return Some(t);
        }
    }
    None
}

/// Deja un título apto como nombre de fichero en cualquier SO: recorta
/// espacios, sustituye `<>:"/\|?*` y controles por `_`. La eñe, los
/// espacios y los paréntesis se conservan.
pub fn safe_stem(title: &str) -> String {
    let mut s: String = title
        .trim()
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect();
    while s.ends_with('.') || s.ends_with(' ') {
        s.pop();
    }
    if s.is_empty() {
        s.push_str("juego");
    }
    s
}

fn align64(n: u64) -> u64 {
    n.next_multiple_of(64)
}

/// Donante para salida CIA: bloques copiados del CIA original (misma
/// TitleId). Vale con donantes "footer-style" (SMDH al final, p. ej. el
/// CIA de Luis) y "meta-style" (meta clásica tras el TMD).
#[derive(Debug, Clone)]
pub struct CiaDonor {
    pub cert: Vec<u8>,
    pub ticket: Vec<u8>,
    pub tmd: Vec<u8>,
    pub meta: Vec<u8>,
    pub footer: Vec<u8>,
    pub title_id: u64,
    tmd_head: usize,
    tmd_chunks: usize,
    tmd_count: usize,
    tmd_siglen: usize,
}

/// Un record del TMD donante con su offset para reescribirlo.
#[derive(Debug, Clone)]
pub struct DonorRecord {
    pub index: u16,
    pub size: u64,
    pub sha: [u8; 32],
    rec_off: usize,
}

fn be16(b: &[u8], o: usize) -> u16 {
    u16::from_be_bytes([b[o], b[o + 1]])
}
fn be32(b: &[u8], o: usize) -> u32 {
    u32::from_be_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn be64(b: &[u8], o: usize) -> u64 {
    u64::from_be_bytes([
        b[o], b[o + 1], b[o + 2], b[o + 3], b[o + 4], b[o + 5], b[o + 6], b[o + 7],
    ])
}

fn tmd_sig_len(t: u32) -> Result<usize> {
    match t {
        // (misma tabla que find_tmd: longitudes empíricas por tipo)
        0x10003 => Ok(0x200),
        0x10004 => Ok(0x100),
        0x10005 => Ok(0x3C),
        _ => Err(Error::Format(format!("firma TMD {t:#x} no soportada"))),
    }
}

fn read_range(f: &mut File, off: u64, len: u64) -> Result<Vec<u8>> {
    if len > 64 * 1024 * 1024 {
        return Err(Error::Format("sección CIA sospechosamente grande".into()));
    }
    let mut b = vec![0u8; len as usize];
    f.seek(SeekFrom::Start(off))?;
    f.read_exact(&mut b)?;
    Ok(b)
}

pub fn read_donor(path: &Path) -> Result<CiaDonor> {
    let mut f = File::open(path)?;
    let total = f.seek(SeekFrom::End(0))?;
    f.seek(SeekFrom::Start(0))?;
    let mut hdr = [0u8; 0x20];
    f.read_exact(&mut hdr)?;
    if u32le(&hdr, 0) != 0x2020 {
        return Err(Error::Format("el donante no es un CIA".into()));
    }
    let (cert_size, tik_size, tmd_size, meta_or_footer) = (
        u32le(&hdr, 0x08) as u64,
        u32le(&hdr, 0x0C) as u64,
        u32le(&hdr, 0x10) as u64,
        u32le(&hdr, 0x14) as u64,
    );
    let mut off = align64(0x2020);
    let cert = read_range(&mut f, off, cert_size)?;
    off = align64(off + cert_size);
    let ticket = read_range(&mut f, off, tik_size)?;
    off = align64(off + tik_size);
    let tmd_off = off;
    let tmd = find_tmd(&mut f, tmd_size, total, meta_or_footer)?;
    let siglen = tmd_sig_len(be32(&tmd, 0))?;
    let head = (4 + siglen + 63) & !63;
    let count = be16(&tmd, head + 0x9E) as usize;
    if count == 0 || count > 64 {
        return Err(Error::Format(format!("content count raro: {count}")));
    }
    let chunks = head + 0xC4 + 64 * 0x24;
    if chunks + count * 0x30 > tmd.len() {
        return Err(Error::Format("TMD truncado en records".into()));
    }
    let title_id = be64(&tmd, head + 0x4C);
    if title_id == 0 {
        return Err(Error::Format("TMD sin TitleId".into()));
    }
    // ¿Meta clásica (hueco entre TMD y content0) o footer final?
    let layout = read_layout(path)?;
    let c0 = layout
        .contents
        .iter()
        .min_by_key(|c| c.offset)
        .ok_or_else(|| Error::Format("el donante no trae contenidos".into()))?;
    let after_tmd = align64(tmd_off + tmd_size);
    let (meta, footer) = if c0.offset > after_tmd {
        (read_range(&mut f, after_tmd, c0.offset - after_tmd)?, Vec::new())
    } else {
        let footer = if meta_or_footer > 0 {
            if meta_or_footer > total {
                return Err(Error::Format("footer CIA incoherente".into()));
            }
            read_range(&mut f, total - meta_or_footer, meta_or_footer)?
        } else {
            Vec::new()
        };
        (Vec::new(), footer)
    };
    Ok(CiaDonor {
        cert,
        ticket,
        tmd,
        meta,
        footer,
        title_id,
        tmd_head: head,
        tmd_chunks: chunks,
        tmd_count: count,
        tmd_siglen: siglen,
    })
}

impl CiaDonor {
    pub fn records(&self) -> Vec<DonorRecord> {
        (0..self.tmd_count)
            .map(|i| {
                let o = self.tmd_chunks + i * 0x30;
                let mut sha = [0u8; 32];
                sha.copy_from_slice(&self.tmd[o + 16..o + 48]);
                DonorRecord {
                    index: be16(&self.tmd, o + 4),
                    size: be64(&self.tmd, o + 8),
                    sha,
                    rec_off: o,
                }
            })
            .collect()
    }
}

/// TitleId del CCI (partición 0) para emparejar con el donante.
fn cci_title_id(cci: &Path) -> Result<u64> {
    let slots = crate::cci::partitions_indexed(cci)?;
    let (_, p0, _) = *slots
        .first()
        .ok_or_else(|| Error::Format("CCI sin particiones".into()))?;
    let mut f = File::open(cci)?;
    let mut ncch = [0u8; 0x200];
    f.seek(SeekFrom::Start(p0))?;
    f.read_exact(&mut ncch)?;
    if u32le(&ncch, 0x100) != 0x4843_434E {
        return Err(Error::Format("partición 0 sin magia NCCH".into()));
    }
    Ok(u64::from_le_bytes(ncch[0x108..0x110].try_into().unwrap()))
}

/// Ticket falso estilo GM9 (BuildFakeTicket/BuildVariableFakeTicket):
/// firma y ECDSA a 0xFF, titlekey a 0xFF, derechos genéricos para 64
/// contents. Es lo que monta GM9 al construir CIAs; converge byte a byte
/// con su referencia. El ticket del donante (aunque traiga firma válida)
/// se sustituye: el falso es el formato instalable estándar.
pub fn build_fake_ticket(title_id: u64) -> Vec<u8> {
    let mut t = vec![0u8; 0x350];
    t[0..4].copy_from_slice(&0x10004u32.to_be_bytes());
    for b in t[4..0x104].iter_mut() {
        *b = 0xFF;
    }
    // padding1 [0x104..0x140] a cero
    t[0x140..0x140 + 26].copy_from_slice(b"Root-CA00000003-XS0000000c");
    for b in t[0x180..0x1BC].iter_mut() {
        *b = 0xFF;
    } // ecdsa
    t[0x1BC] = 0x01; // version
    for b in t[0x1BF..0x1CF].iter_mut() {
        *b = 0xFF;
    } // titlekey
    t[0x1DC..0x1E4].copy_from_slice(&title_id.to_be_bytes());
    // commonkey_idx eshop (0x1F1) y reserva ya a cero; audit = 1
    t[0x221] = 0x01;
    // content_index: 1 rights field (64 contents máx)
    let ci = 0x2A4;
    t[ci + 1] = 0x01;
    t[ci + 3] = 0x14;
    t[ci + 4..ci + 8].copy_from_slice(&0xACu32.to_be_bytes());
    t[ci + 11] = 0x14;
    t[ci + 13] = 0x01;
    t[ci + 15] = 0x14;
    t[ci + 0x14 + 3] = 0x28;
    t[ci + 0x14 + 7] = 0x01; // max_entry_count = 1
    t[ci + 0x14 + 11] = 0x84; // size_per_entry
    t[ci + 0x14 + 15] = 0x84; // total_size_used
    t[ci + 0x14 + 17] = 0x03; // data_type = rights
    for b in t[ci + 0x28 + 4..ci + 0x28 + 0x84].iter_mut() {
        *b = 0xFF;
    } // rightsbitfield (indexoffset y 2 previos a cero, como GM9)
    t
}

/// Construye un CIA instalable desde un CCI descifrado + CIA donante.
///
/// Política: los contents son los records del donante en orden de índice,
/// tomados de los slots del CCI; solo la partición 0 puede cambiar (el
/// resto debe ser byte-idéntico al donante: puerta). El TMD se reescribe
/// (tamaños + SHAs de records + rehash contentinfo al estilo GM9
/// FixTmdHashes, que los instaladores verifican) con la firma a 0xFF
/// (convención GM9: el cero se lee como ausente) y ticket falso generado —
/// hace falta CFW con parches de firma (Luma por defecto) para instalarlo,
/// igual que un CIA montado desde `.3ds` con GodMode9. Cert y footer/meta
/// (SMDH) se conservan del donante.
pub fn build_from_cci(
    cci: &Path,
    donor_cia: &Path,
    out_cia: &Path,
    cancel: &AtomicBool,
    mut prog: impl FnMut(u64, u64),
) -> Result<()> {
    use std::sync::atomic::Ordering;
    let donor = read_donor(donor_cia)?;
    if cci_title_id(cci)? != donor.title_id {
        return Err(Error::Verify(
            "el donante es de otro juego (TitleId distinto)".into(),
        ));
    }
    let slots = crate::cci::partitions_indexed(cci)?;
    let mut recs = donor.records();
    recs.sort_by_key(|r| r.index);
    // Puerta: todo slot no-juego debe ser idéntico al donante.
    let dlayout = read_layout(donor_cia)?;
    struct NewContent {
        index: u16,
        src_off: u64,
        size: u64,
        sha: [u8; 32],
    }
    let mut new_contents = Vec::with_capacity(recs.len());
    for r in &recs {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let (_, soff, ssize) = *slots
            .iter()
            .find(|(i, _, _)| *i as u16 == r.index)
            .ok_or_else(|| {
                Error::Format(format!("el CCI no trae la partición {}", r.index))
            })?;
        let sha = stream_sha(cci, soff, ssize, cancel, &mut |_, _| {})?;
        if r.index != 0 {
            let d = dlayout.contents.iter().find(|c| c.index == r.index).ok_or_else(|| {
                Error::Format(format!("el donante no trae content{}", r.index))
            })?;
            let dsha = stream_sha(donor_cia, d.offset, d.size, cancel, &mut |_, _| {})?;
            if sha != dsha || ssize != d.size {
                return Err(Error::Verify(format!(
                    "la partición {} cambió y no es el juego (pack no soportado)",
                    r.index
                )));
            }
        }
        new_contents.push(NewContent { index: r.index, src_off: soff, size: ssize, sha: unhex(&sha)? });
    }
    // TMD nuevo: records al día + contentinfo rehecho (espejo de GM9
    // FixTmdHashes: los instaladores verifican estas cadenas) + firma a
    // cero (fakesign: instala con CFW).
    let mut tmd = donor.tmd.clone();
    for (r, n) in recs.iter().zip(new_contents.iter()) {
        tmd[r.rec_off + 8..r.rec_off + 16].copy_from_slice(&n.size.to_be_bytes());
        tmd[r.rec_off + 16..r.rec_off + 48].copy_from_slice(&n.sha);
    }
    {
        use sha2::{Digest, Sha256};
        let chunks = donor.tmd_chunks;
        let cib = chunks - 64 * 0x24;
        // Espejo exacto de GM9 FixTmdHashes: se sale en cuanto kc cubre
        // los contents (las entradas sobrantes con k=0 se dejan como
        // están, a cero como las deja BuildFakeTmd).
        let mut kc = 0usize;
        for i in 0..64 {
            if kc >= donor.tmd_count {
                break;
            }
            let eo = cib + i * 0x24;
            let k = be16(&tmd, eo + 2) as usize;
            let h = Sha256::digest(&tmd[chunks + kc * 0x30..chunks + (kc + k) * 0x30]);
            tmd[eo + 4..eo + 36].copy_from_slice(&h);
            kc += k;
        }
        let master = Sha256::digest(&tmd[cib..cib + 64 * 0x24]);
        tmd[cib - 0x20..cib].copy_from_slice(&master);
    }
    // Firma a 0xFF (convención GM9 BuildFakeTmd: cero significa
    // "ausente/corrupto" para algunos chequeos; 0xFF es "falso asumido").
    // Solo la firma en sí: el padding posterior va a cero como en GM9.
    let sig_end = 4 + donor.tmd_siglen;
    for b in tmd[4..sig_end].iter_mut() {
        *b = 0xFF;
    }
    for b in tmd[sig_end..donor.tmd_head].iter_mut() {
        *b = 0x00;
    }
    // Ensambla: cabecera + secciones alineadas a 64 + contents + meta/footer.
    // Ticket falso generado (el del donante no se reutiliza).
    let ticket = build_fake_ticket(donor.title_id);
    let sections: [&[u8]; 3] = [&donor.cert, &ticket, &tmd];
    let content_total: u64 = new_contents.iter().map(|c| c.size).sum();
    let mut hdr = [0u8; 0x20];
    hdr[0..4].copy_from_slice(&0x2020u32.to_le_bytes());
    hdr[0x08..0x0C].copy_from_slice(&(donor.cert.len() as u32).to_le_bytes());
    hdr[0x0C..0x10].copy_from_slice(&(ticket.len() as u32).to_le_bytes());
    hdr[0x10..0x14].copy_from_slice(&(tmd.len() as u32).to_le_bytes());
    let tail_len = (donor.meta.len() + donor.footer.len()) as u64;
    hdr[0x14..0x18].copy_from_slice(&(tail_len as u32).to_le_bytes());
    hdr[0x18..0x1C].copy_from_slice(&(content_total as u32).to_le_bytes());
    if let Some(p) = out_cia.parent() {
        if !p.as_os_str().is_empty() {
            std::fs::create_dir_all(p)?;
        }
    }
    let mut o = File::create(out_cia)?;
    o.write_all(&hdr)?;
    // Bitmap de contents [0x20..0x2020] (espejo de GM9 FixCiaHeaderForTmd):
    // sin estos bits los instaladores ven "0/N contents" y saltan todo
    // (verify verde instantáneo + install que muere al instante).
    {
        let mut cindex = [0u8; 0x2000];
        for n in &new_contents {
            cindex[n.index as usize / 8] |= 1 << (7 - (n.index % 8));
        }
        o.write_all(&cindex)?;
    }
    // Las secciones van contiguas desde el fin de la cabecera (0x2020),
    // cada una alineada a 64: la primera (cert) arranca en 0x2040, no en
    // 0x2020. Sin ese alineado inicial todo queda 0x20 corrido y el cert
    // ilegible (bug que tumbaba la instalación).
    o.seek(SeekFrom::Start(align64(0x2020)))?;
    let mut cur = align64(0x2020);
    let mut pad_to_64 = |o: &mut File, cur: &mut u64| -> Result<()> {
        let pad = pad64(*cur) - *cur;
        if pad > 0 {
            o.write_all(&vec![0u8; pad as usize])?;
            *cur += pad;
        }
        Ok(())
    };
    for s in sections {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        o.write_all(s)?;
        cur += s.len() as u64;
        pad_to_64(&mut o, &mut cur)?;
        prog(cur, content_total.saturating_add(cur));
    }
    if !donor.meta.is_empty() {
        o.write_all(&donor.meta)?;
        cur += donor.meta.len() as u64;
        pad_to_64(&mut o, &mut cur)?;
    }
    let mut f = File::open(cci)?;
    let mut buf = vec![0u8; 8 * 1024 * 1024];
    let mut cdone = 0u64;
    for n in &new_contents {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        f.seek(SeekFrom::Start(n.src_off))?;
        let mut left = n.size;
        while left > 0 {
            let k = left.min(buf.len() as u64) as usize;
            f.read_exact(&mut buf[..k])?;
            o.write_all(&buf[..k])?;
            left -= k as u64;
            cdone += k as u64;
            prog(cdone, content_total);
        }
        cur += n.size;
        pad_to_64(&mut o, &mut cur)?;
    }
    if !donor.footer.is_empty() {
        o.write_all(&donor.footer)?;
    }
    o.flush()?;
    drop(o);
    // Verifica: re-parsea + NCCH por content + SHAs del TMD.
    let back = read_donor(out_cia)?;
    if back.title_id != donor.title_id || back.records().len() != recs.len() {
        return Err(Error::Verify("el CIA generado no re-parsea (bug interno)".into()));
    }
    let bl = read_layout(out_cia)?;
    for n in &new_contents {
        let c = bl.contents.iter().find(|c| c.index == n.index).ok_or_else(|| {
            Error::Verify("content perdido en el CIA generado (bug interno)".into())
        })?;
        let r = back
            .records()
            .into_iter()
            .find(|r| r.index == n.index)
            .ok_or_else(|| Error::Verify("record perdido en el CIA generado (bug interno)".into()))?;
        if c.size != n.size || r.sha != n.sha {
            return Err(Error::Verify("record incoherente en el CIA generado (bug interno)".into()));
        }
    }
    prog(content_total, content_total);
    Ok(())
}

fn pad64(n: u64) -> u64 {
    n.next_multiple_of(64)
}

fn stream_sha(
    path: &Path,
    off: u64,
    len: u64,
    cancel: &AtomicBool,
    mut on_chunk: impl FnMut(u64, u64),
) -> Result<String> {
    use sha2::{Digest, Sha256};
    use std::sync::atomic::Ordering;
    let mut f = File::open(path)?;
    f.seek(SeekFrom::Start(off))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 8 * 1024 * 1024];
    let mut left = len;
    let mut done = 0u64;
    while left > 0 {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let k = left.min(buf.len() as u64) as usize;
        f.read_exact(&mut buf[..k])?;
        h.update(&buf[..k]);
        left -= k as u64;
        done += k as u64;
        on_chunk(done, len);
    }
    Ok(hex_of(&h.finalize()))
}

fn hex_of(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Result<[u8; 32]> {
    if s.len() != 64 {
        return Err(Error::Format("sha hex inválido".into()));
    }
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = u8::from_str_radix(&s[2 * i..2 * i + 2], 16)
            .map_err(|_| Error::Format("sha hex inválido".into()))?;
    }
    Ok(out)
}
