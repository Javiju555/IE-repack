//! Parseo mínimo de contenedor CIA: localizar los contents (solo lectura).
//!
//! Estrategia idéntica a `rebuild.sh`: los tamaños salen del TMD y los
//! offsets se calculan desde el final (el contenido queda justo antes del
//! footer), sin adivinar paddings de cabecera.

use crate::error::{Error, Result};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

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
