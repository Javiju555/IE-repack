// SPDX-FileCopyrightText: 2026 Javiju
//
// SPDX-License-Identifier: MIT

//! Localización y reconstrucción de ficheros dentro del RomFS.
//!
//! El RomFS de estos juegos no se parsea por cabecera, sino por
//! **verificación de contenido**: cada fichero del manifiesto trae su
//! SHA-256 original, así que una entrada de tabla solo se acepta si los
//! bytes a los que apunta cuadran al byte. Esto hace de puerta estricta
//! (base equivocada => error, nunca quimera).
//!
//! La base del área de datos (FileData) se descubre probando candidatos
//! contra el fichero más pequeño del manifiesto: el único B que verifica
//! es el bueno (colisión accidental de SHA-256 ≈ imposible).
//!
//! Estrategia de rebuild COMPLETO con rehash IVFC: se reempaqueta el nivel
//! de datos entero (tablas intactas salvo doff/size, ficheros en orden
//! original) y se recalcula la cadena lvl2->lvl1->master desde cero, con
//! tamaños coherentes. Es lo que verifica GodMode9 a fondo
//! (`VerifyNcchFile`: master vs lvl1, lvl1 vs lvl2, streaming lvl3 vs lvl2);
//! el anterior modo apéndice dejaba hashes rancios y no pasaba.
//!
//! Todas las funciones trabajan sobre el fichero contenedor (`src`) con
//! `src_base` = offset donde empieza el RomFS dentro de él; los offsets de
//! tabla y FileData son relativos al RomFS, como los guarda el formato.

use crate::error::{Error, Result};
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Entrada FileMeta localizada y verificada (offsets relativos al RomFS).
#[derive(Debug, Clone)]
pub struct Located {
    /// Offset en el blob RomFS donde empieza la entrada de tabla.
    pub table_off: u64,
    /// Índice de directorio padre (tal cual lo guarda la tabla).
    pub parent: u32,
    /// Offset relativo a FileData donde empiezan los datos.
    pub data_off: u64,
    pub len: u64,
}

/// Dónde ha quedado un fichero en el blob reconstruido (para verificar).
#[derive(Debug, Clone)]
pub struct NewLoc {
    pub table_off: u64,
    pub data_off: u64,
    pub len: u64,
}

fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn u64le(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        return Err(Error::Cancelled);
    }
    Ok(())
}

/// SHA-256 de un rango absoluto [off, off+len) del fichero, en streaming.
pub fn sha256_range(
    path: &Path,
    off: u64,
    len: u64,
    cancel: &AtomicBool,
    mut on_chunk: impl FnMut(u64),
) -> Result<String> {
    let mut f = std::fs::File::open(path)?;
    f.seek(SeekFrom::Start(off))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 8 * 1024 * 1024];
    let mut left = len;
    while left > 0 {
        check_cancel(cancel)?;
        let n = left.min(buf.len() as u64) as usize;
        f.read_exact(&mut buf[..n])?;
        h.update(&buf[..n]);
        left -= n as u64;
        on_chunk(n as u64);
    }
    Ok(format!("{:x}", h.finalize()))
}

/// Lee la región de tablas (principio del RomFS). Límite generoso pero
/// acotado: las tablas reales ocupan ~100 KB; si no bastan, se avisa.
pub fn read_tables(src: &Path, src_base: u64, romfs_len: u64) -> Result<Vec<u8>> {
    const MAX: u64 = 8 * 1024 * 1024;
    let n = romfs_len.min(MAX);
    let mut f = std::fs::File::open(src)?;
    f.seek(SeekFrom::Start(src_base))?;
    let mut buf = vec![0u8; n as usize];
    f.read_exact(&mut buf)?;
    Ok(buf)
}

/// Candidatos de tabla por nombre UTF-16 + tamaño.
pub fn find_candidates(tables: &[u8], name: &str, size: u64) -> Vec<Located> {
    let needle: Vec<u8> = name.encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
    let mut out = Vec::new();
    if needle.is_empty() || needle.len() + 0x20 > tables.len() {
        return out;
    }
    let mut s = 0;
    while s + needle.len() <= tables.len() {
        let rel = tables[s..]
            .windows(needle.len())
            .position(|w| w == needle.as_slice());
        let h = match rel {
            Some(r) => s + r,
            None => break,
        };
        s = h + 1;
        if h < 0x20 {
            continue;
        }
        let e = h - 0x20;
        let parent = u32le(tables, e);
        let data_off = u64le(tables, e + 8);
        let len = u64le(tables, e + 16);
        let nlen = u32le(tables, e + 28) as usize;
        if nlen != needle.len() || len != size {
            continue;
        }
        if tables[e + 0x20..e + 0x20 + nlen] != needle {
            continue;
        }
        if data_off > 8 * 1024 * 1024 * 1024 {
            continue;
        }
        out.push(Located {
            table_off: e as u64,
            parent,
            data_off,
            len,
        });
    }
    out
}

/// Descubre la base de FileData: el único B que verifica el SHA del
/// fichero pequeño en alguno de sus candidatos.
#[allow(clippy::too_many_arguments)]
pub fn discover_file_data_base(
    src: &Path,
    src_base: u64,
    romfs_len: u64,
    small_size: u64,
    small_sha: &str,
    small_cands: &[Located],
    cancel: &AtomicBool,
    mut on_try: impl FnMut(u64),
) -> Result<u64> {
    if small_cands.is_empty() {
        return Err(Error::Verify(
            "sin entradas candidatas en la base (¿juego equivocado?)".into(),
        ));
    }
    // B vive tras las tablas (típico ~100 KB); se prueba de abajo arriba.
    let mut b = 0x100u64;
    // Cota: las tablas están dentro de los primeros MB; 4 MB es generoso.
    while b < 0x400000u64 {
        check_cancel(cancel)?;
        on_try(b);
        for c in small_cands {
            if c.len != small_size {
                continue;
            }
            let abs = src_base.saturating_add(b).saturating_add(c.data_off);
            if abs.saturating_add(c.len) > src_base.saturating_add(romfs_len) {
                continue;
            }
            let got = sha256_range(src, abs, c.len, cancel, &mut |_| {})?;
            if got == small_sha {
                return Ok(b);
            }
        }
        b += 16;
    }
    Err(Error::Verify(
        "ningún origen cuadra con el manifiesto (¿base distinta a la del parche?)".into(),
    ))
}

/// Resuelve un fichero a sus entradas verificadas en B (todas las que
/// cuadren; normalmente una; duplicados de tabla, varias).
#[allow(clippy::too_many_arguments)]
pub fn resolve_verified(
    src: &Path,
    src_base: u64,
    romfs_len: u64,
    file_data_base: u64,
    cands: &[Located],
    want_sha: &str,
    ruta: &str,
    cancel: &AtomicBool,
) -> Result<Vec<Located>> {
    let mut ok = Vec::new();
    for c in cands {
        check_cancel(cancel)?;
        let abs = src_base
            .saturating_add(file_data_base)
            .saturating_add(c.data_off);
        if abs.saturating_add(c.len) > src_base.saturating_add(romfs_len) {
            continue;
        }
        let got = sha256_range(src, abs, c.len, cancel, &mut |_| {})?;
        if got == want_sha {
            ok.push(c.clone());
        }
    }
    if ok.is_empty() {
        return Err(Error::Verify(format!(
            "{ruta}: el original no cuadra (¿otra versión del juego?)"
        )));
    }
    Ok(ok)
}

/// Un reemplazo ya decodificado y verificado, listo para apendizar.
pub struct Replacement {
    /// Entradas de tabla verificadas (todas apuntan a los mismos bytes).
    pub locs: Vec<Located>,
    /// Fichero temporal con los bytes nuevos (resultado del xdelta).
    pub new_file: PathBuf,
    pub new_len: u64,
}

/// Reconstruye el blob RomFS en `out`: copia [0, len) parcheando solo las
/// entradas mapeadas (doff -> apéndice, dlen -> nueva) y apendiza los
/// payloads nuevos al final (alineados a 0x200). Streaming con progreso.
///
/// Devuelve el layout nuevo (para verificar los SHA resultado).
#[allow(clippy::too_many_arguments)]
pub fn rebuild_append_only(
    src: &Path,
    src_base: u64,
    romfs_len: u64,
    file_data_base: u64,
    repl: &[Replacement],
    out: &Path,
    cancel: &AtomicBool,
    mut on_chunk: impl FnMut(u64, u64),
) -> Result<Vec<NewLoc>> {
    // 1) Cabecera/tablas en RAM (file_data_base < 4 MB por descubrimiento).
    let mut head = vec![0u8; file_data_base as usize];
    {
        let mut f = std::fs::File::open(src)?;
        f.seek(SeekFrom::Start(src_base))?;
        f.read_exact(&mut head)?;
    }
    // 2) Reserva posiciones de apéndice (todas conocidas por adelantado).
    // Si dos entradas verificadas comparten doff, comparten apéndice.
    let mut cursor = romfs_len;
    // doff_antiguo -> (at_nuevo_absoluto_en_blob, new_len)
    let mut appends: std::collections::BTreeMap<u64, (u64, u64)> = Default::default();
    for r in repl {
        for l in &r.locs {
            appends.entry(l.data_off).or_insert_with(|| {
                cursor = cursor.next_multiple_of(0x200);
                let at = cursor;
                cursor += r.new_len;
                (at, r.new_len)
            });
        }
    }
    // 3) Parchea las entradas en la copia de cabecera.
    let mut layout = Vec::new();
    for r in repl {
        for l in &r.locs {
            let (at, nl) = appends[&l.data_off];
            if nl != r.new_len {
                return Err(Error::Format(
                    "parches en conflicto sobre el mismo bloque".into(),
                ));
            }
            let e = l.table_off as usize;
            if e + 24 > head.len() {
                return Err(Error::Format("entrada fuera de la región de tablas".into()));
            }
            head[e + 8..e + 16].copy_from_slice(&(at - file_data_base).to_le_bytes());
            head[e + 16..e + 24].copy_from_slice(&nl.to_le_bytes());
            layout.push(NewLoc {
                table_off: l.table_off,
                data_off: at - file_data_base,
                len: nl,
            });
        }
    }
    // 4) Escribe: cabecera + resto original + apéndices (streaming).
    let total = cursor;
    let mut o = std::fs::File::create(out)?;
    let mut done = 0u64;
    o.write_all(&head)?;
    done += head.len() as u64;
    on_chunk(done, total);
    {
        let mut f = std::fs::File::open(src)?;
        f.seek(SeekFrom::Start(src_base + file_data_base))?;
        let mut buf = vec![0u8; 8 * 1024 * 1024];
        let mut left = romfs_len - file_data_base;
        while left > 0 {
            check_cancel(cancel)?;
            let n = left.min(buf.len() as u64) as usize;
            f.read_exact(&mut buf[..n])?;
            o.write_all(&buf[..n])?;
            left -= n as u64;
            done += n as u64;
            on_chunk(done, total);
        }
    }
    // Orden de apéndice = orden de doff antiguo (determinista).
    let mut ordered: Vec<(u64, u64, &Replacement)> = Vec::new();
    for r in repl {
        let d0 = r.locs.first().map(|l| l.data_off).unwrap_or(u64::MAX);
        ordered.push((d0, appends[&d0].0, r));
    }
    ordered.sort_by_key(|(d, _, _)| *d);
    let mut seen: Vec<u64> = Vec::new();
    for (_, at, r) in ordered {
        check_cancel(cancel)?;
        // El mismo doff compartido se escribe una sola vez.
        let key = r.locs.first().map(|l| l.data_off).unwrap_or(u64::MAX);
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        while done < at {
            let z = (at - done).min(8 * 1024 * 1024) as usize;
            o.write_all(&vec![0u8; z])?;
            done += z as u64;
            on_chunk(done, total);
        }
        let mut f = std::fs::File::open(&r.new_file)?;
        let mut buf = vec![0u8; 8 * 1024 * 1024];
        let mut left = r.new_len;
        while left > 0 {
            check_cancel(cancel)?;
            let n = left.min(buf.len() as u64) as usize;
            f.read_exact(&mut buf[..n])?;
            o.write_all(&buf[..n])?;
            left -= n as u64;
            done += n as u64;
            on_chunk(done, total);
        }
    }
    o.flush()?;
    on_chunk(total, total);
    // Cierra alineado a unidad de media (el NCCH guarda el tamaño en medias).
    let aligned = total.next_multiple_of(0x200);
    if aligned != total {
        let mut o = std::fs::OpenOptions::new().append(true).open(out)?;
        o.write_all(&vec![0u8; (aligned - total) as usize])?;
        o.flush()?;
        on_chunk(aligned, aligned);
    }
    Ok(layout)
}

/// Offset en disco de cada nivel IVFC, espejo de `GetRomFsLvOffset` de
/// GodMode9 (romfs.c): cabecera+master, datos(lvl3), lvl1, lvl2.
fn lv_offset(sm: u64, s1: u64, l1: u32, s2: u64, l2: u32, s3: u64, l3: u32, lvl: u32) -> u64 {
    const HDR: u64 = 0x60;
    let al = |v: u64, o: u32| v.next_multiple_of(1u64 << o);
    let off3 = al(HDR + sm, l3);
    let off1 = off3 + al(s3, l3);
    let off2 = off1 + al(s1, l1);
    let off0 = off2 + al(s2, l2);
    match lvl {
        0 => off0,
        1 => off1,
        2 => off2,
        _ => off3,
    }
}

/// Resultado del rebuild completo: layout para verificar + hash base del
/// RomFS (el NCCH lo guarda en 0x1E0 sobre `romfs_hash_units` medias).
pub struct FullLayout {
    pub locs: Vec<NewLoc>,
    pub new_len: u64,
    pub hash_romfs: [u8; 32],
    pub romfs_hash_units: u32,
}

/// Entrada FileMeta parseada del nivel de datos (offsets relativos al RomFS).
struct MetaEntry {
    /// Offset de la entrada dentro del blob RomFS (como `Located`).
    table_off: u64,
    data_off: u64,
    len: u64,
}

/// Reconstruye el RomFS entero con rehash IVFC (ver módulo): reempaqueta los
/// datos en orden original (tablas iguales salvo doff/size), recalcula
/// lvl2/lvl1/master y deja tamaños coherentes. `repl` igual que en apéndice.
#[allow(clippy::too_many_arguments)]
pub fn rebuild_full(
    src: &Path,
    src_base: u64,
    _romfs_len: u64,
    file_data_base: u64,
    repl: &[Replacement],
    out: &Path,
    cancel: &AtomicBool,
    mut on_chunk: impl FnMut(u64, u64),
) -> Result<FullLayout> {
    use std::collections::BTreeMap;
    // 1) Superblock + cabecera del nivel de datos.
    let mut sb = [0u8; 0x60];
    {
        let mut f = std::fs::File::open(src)?;
        f.seek(SeekFrom::Start(src_base))?;
        f.read_exact(&mut sb)?;
    }
    if &sb[0..8] != b"IVFC\x00\x00\x01\x00" {
        return Err(Error::Format("RomFS sin magia IVFC (¿base válida?)".into()));
    }
    let sm = u32le(&sb, 8) as u64;
    let (_s1, l1) = (u64le(&sb, 20), u32le(&sb, 28));
    let (_s2, l2) = (u64le(&sb, 44), u32le(&sb, 52));
    let (_s3, l3) = (u64le(&sb, 68), u32le(&sb, 76));
    for &l in &[l1, l2, l3] {
        if l < 9 || l > 16 {
            return Err(Error::Format("IVFC con bloques raros (¿base válida?)".into()));
        }
    }
    let lo3 = lv_offset(sm, _s1, l1, _s2, l2, _s3, l3, 3);
    let mut lh = [0u8; 0x28];
    {
        let mut f = std::fs::File::open(src)?;
        f.seek(SeekFrom::Start(src_base + lo3))?;
        f.read_exact(&mut lh)?;
    }
    if u32le(&lh, 0) != 0x28 {
        return Err(Error::Format("cabecera de datos RomFS rara (¿base válida?)".into()));
    }
    let fmo = u32le(&lh, 28) as u64;
    let fms = u32le(&lh, 32) as u64;
    let fdo = u32le(&lh, 36) as u64;
    if file_data_base != lo3 + fdo {
        return Err(Error::Format("la base de datos no cuadra con la cabecera (bug interno)".into()));
    }
    // 2) Camina la FileMeta lineal (entradas empaquetadas).
    let mut fm = vec![0u8; fms as usize];
    {
        let mut f = std::fs::File::open(src)?;
        f.seek(SeekFrom::Start(src_base + lo3 + fmo))?;
        f.read_exact(&mut fm)?;
    }
    let mut entries: Vec<MetaEntry> = Vec::new();
    let mut p = 0usize;
    while p < fm.len() {
        if p + 0x20 > fm.len() {
            return Err(Error::Format("FileMeta truncada (¿base válida?)".into()));
        }
        let nlen = u32le(&fm, p + 28) as usize;
        // nlen = bytes UTF-16 del nombre; la entrada rellena a múltiplo de 4.
        if nlen > 512 || p + 0x20 + nlen > fm.len() {
            return Err(Error::Format("FileMeta corrupta (¿base válida?)".into()));
        }
        let (doff, len) = (u64le(&fm, p + 8), u64le(&fm, p + 16));
        if fdo + doff + len > _s3 {
            return Err(Error::Format("entrada fuera de los datos (¿base válida?)".into()));
        }
        entries.push(MetaEntry { table_off: lo3 + fmo + p as u64, data_off: doff, len });
        // Las entradas se rellenan a múltiplo de 4.
        p += (0x20 + nlen).next_multiple_of(4);
    }
    if entries.is_empty() {
        return Err(Error::Format("RomFS sin ficheros (¿base válida?)".into()));
    }
    // 3) Mapa de reemplazos por doff antiguo + nuevo empaquetado en orden.
    let mut rep_by_doff: BTreeMap<u64, &Replacement> = BTreeMap::new();
    for r in repl {
        for l in &r.locs {
            let prev = rep_by_doff.insert(l.data_off, r);
            if prev.is_some_and(|q| q.new_len != r.new_len) {
                return Err(Error::Format("parches en conflicto sobre el mismo bloque".into()));
            }
        }
    }
    // doff_antiguo -> (doff_nuevo_relativo_a_FileData, len_nueva)
    let mut placed: BTreeMap<u64, (u64, u64)> = BTreeMap::new();
    let mut cursor = 0u64;
    for e in &entries {
        placed.entry(e.data_off).or_insert_with(|| {
            let nl = rep_by_doff.get(&e.data_off).map(|r| r.new_len).unwrap_or(e.len);
            let at = cursor;
            cursor += nl;
            (at, nl)
        });
    }
    let s3n = fdo + cursor;
    // 4) Nuevos tamaños de niveles (mismos órdenes de bloque).
    let e3 = s3n.next_multiple_of(1u64 << l3) >> l3;
    let s2n = e3 * 0x20;
    let b2 = s2n.next_multiple_of(1u64 << l2) >> l2;
    let s1n = b2 * 0x20;
    let b1 = s1n.next_multiple_of(1u64 << l1) >> l1;
    let smn = b1 * 0x20;
    let no3 = lv_offset(smn, s1n, l1, s2n, l2, s3n, l3, 3);
    let no1 = lv_offset(smn, s1n, l1, s2n, l2, s3n, l3, 1);
    let no2 = lv_offset(smn, s1n, l1, s2n, l2, s3n, l3, 2);
    let total = lv_offset(smn, s1n, l1, s2n, l2, s3n, l3, 0).next_multiple_of(0x200);
    let roh_units = ((0x60 + smn).next_multiple_of(0x200) / 0x200) as u32;
    // 5) Escritura: superblock + master(0) + datos + lvl1(0) + lvl2(0).
    let mut locs = Vec::new();
    let prog_total = total + s3n + s2n + s1n;
    {
        let mut o = std::fs::File::create(out)?;
        let mut done: u64;
        // Superblock con tamaños nuevos y offsets reales.
        let mut nsb = [0u8; 0x60];
        nsb[0..8].copy_from_slice(b"IVFC\x00\x00\x01\x00");
        nsb[8..12].copy_from_slice(&(smn as u32).to_le_bytes());
        nsb[12..20].copy_from_slice(&no1.to_le_bytes());
        nsb[20..28].copy_from_slice(&s1n.to_le_bytes());
        nsb[28..32].copy_from_slice(&l1.to_le_bytes());
        nsb[36..44].copy_from_slice(&no2.to_le_bytes());
        nsb[44..52].copy_from_slice(&s2n.to_le_bytes());
        nsb[52..56].copy_from_slice(&l2.to_le_bytes());
        nsb[60..68].copy_from_slice(&no3.to_le_bytes());
        nsb[68..76].copy_from_slice(&s3n.to_le_bytes());
        nsb[76..80].copy_from_slice(&l3.to_le_bytes());
        // unknown0/1 + hueco: se conservan los originales.
        nsb[84..0x60].copy_from_slice(&sb[84..0x60]);
        o.write_all(&nsb)?;
        o.write_all(&vec![0u8; smn as usize])?;
        let mut pad = no3 - (0x60 + smn);
        let z = vec![0u8; 8 * 1024 * 1024];
        while pad > 0 {
            check_cancel(cancel)?;
            let n = pad.min(z.len() as u64) as usize;
            o.write_all(&z[..n])?;
            pad -= n as u64;
        }
        done = no3;
        on_chunk(done, prog_total);
        // Tablas con entradas parcheadas (copia en RAM: ~1.4 MB).
        let mut tables = vec![0u8; fdo as usize];
        {
            let mut f = std::fs::File::open(src)?;
            f.seek(SeekFrom::Start(src_base + lo3))?;
            f.read_exact(&mut tables)?;
        }
        for e in &entries {
            let (at, nl) = placed[&e.data_off];
            let te = (e.table_off - lo3) as usize;
            tables[te + 8..te + 16].copy_from_slice(&at.to_le_bytes());
            tables[te + 16..te + 24].copy_from_slice(&nl.to_le_bytes());
            // El layout solo cubre entradas reemplazadas (como el apéndice).
            if rep_by_doff.contains_key(&e.data_off) {
                locs.push(NewLoc { table_off: e.table_off, data_off: at, len: nl });
            }
        }
        locs.sort_by_key(|l| (l.data_off, l.table_off));
        locs.dedup_by_key(|l| (l.data_off, l.table_off));
        o.write_all(&tables)?;
        done += fdo;
        // Datos de fichero en orden de empaquetado.
        let mut order: Vec<(u64, u64, u64)> = Vec::new(); // (at, len, doff_viejo)
        for (&d0, &(at, nl)) in &placed {
            order.push((at, nl, d0));
        }
        order.sort_by_key(|(at, _, _)| *at);
        let mut buf = vec![0u8; 8 * 1024 * 1024];
        for (at, nl, d0) in order {
            check_cancel(cancel)?;
            debug_assert_eq!(done, no3 + fdo + at);
            if let Some(r) = rep_by_doff.get(&d0) {
                let mut f = std::fs::File::open(&r.new_file)?;
                let mut left = nl;
                while left > 0 {
                    check_cancel(cancel)?;
                    let n = left.min(buf.len() as u64) as usize;
                    f.read_exact(&mut buf[..n])?;
                    o.write_all(&buf[..n])?;
                    left -= n as u64;
                    done += n as u64;
                }
            } else {
                let old_entry = entries.iter().find(|e| e.data_off == d0).unwrap();
                let mut f = std::fs::File::open(src)?;
                f.seek(SeekFrom::Start(src_base + file_data_base + d0))?;
                let mut left = old_entry.len;
                debug_assert_eq!(left, nl);
                while left > 0 {
                    check_cancel(cancel)?;
                    let n = left.min(buf.len() as u64) as usize;
                    f.read_exact(&mut buf[..n])?;
                    o.write_all(&buf[..n])?;
                    left -= n as u64;
                    done += n as u64;
                }
            }
            on_chunk(done, prog_total);
        }
        // Relleno hasta cubrir s3n + niveles en cero.
        while done < no1 {
            check_cancel(cancel)?;
            let n = (no1 - done).min(z.len() as u64) as usize;
            o.write_all(&z[..n])?;
            done += n as u64;
        }
        while done < total {
            check_cancel(cancel)?;
            let n = (total - done).min(z.len() as u64) as usize;
            o.write_all(&z[..n])?;
            done += n as u64;
        }
        o.flush()?;
        on_chunk(done, prog_total);
    }
    // 6) Rehash bottom-up sobre el propio fichero de salida.
    let mut f = std::fs::OpenOptions::new().read(true).write(true).open(out)?;
    let mut blk = vec![0u8; 1 << l3];
    for i in 0..e3 {
        check_cancel(cancel)?;
        f.seek(SeekFrom::Start(no3 + i * (1u64 << l3)))?;
        f.read_exact(&mut blk)?;
        let h = Sha256::digest(&blk);
        f.seek(SeekFrom::Start(no2 + i * 0x20))?;
        f.write_all(&h)?;
    }
    let mut blk2 = vec![0u8; 1 << l2];
    for i in 0..b2 {
        check_cancel(cancel)?;
        f.seek(SeekFrom::Start(no2 + i * (1u64 << l2)))?;
        f.read_exact(&mut blk2)?;
        let h = Sha256::digest(&blk2);
        f.seek(SeekFrom::Start(no1 + i * 0x20))?;
        f.write_all(&h)?;
    }
    let mut blk1 = vec![0u8; 1 << l1];
    for i in 0..b1 {
        check_cancel(cancel)?;
        f.seek(SeekFrom::Start(no1 + i * (1u64 << l1)))?;
        f.read_exact(&mut blk1)?;
        let h = Sha256::digest(&blk1);
        f.seek(SeekFrom::Start(0x60 + i * 0x20))?;
        f.write_all(&h)?;
    }
    f.flush()?;
    // 7) Hash base del RomFS (superblock+master).
    let mut hb = Sha256::new();
    {
        let mut rf = std::fs::File::open(out)?;
        let mut left = roh_units as u64 * 0x200;
        let mut buf = vec![0u8; 8 * 1024 * 1024];
        while left > 0 {
            check_cancel(cancel)?;
            let n = left.min(buf.len() as u64) as usize;
            rf.read_exact(&mut buf[..n])?;
            hb.update(&buf[..n]);
            left -= n as u64;
        }
    }
    let mut hash_romfs = [0u8; 32];
    hash_romfs.copy_from_slice(&hb.finalize());
    on_chunk(prog_total, prog_total);
    Ok(FullLayout { locs, new_len: total, hash_romfs, romfs_hash_units: roh_units })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Blob sintético: tablas [entrada (name,size,doff)] + datos.
    /// file_data_base = 0x100.
    fn synth() -> (Vec<u8>, u64) {
        let base = 0x100u64;
        let mut t = vec![0u8; base as usize];
        // entrada en off 0x20: parent=0, sib=0, doff=0, len=5, hnext=0, nlen=6 ("abZ")
        let mut e = [0u8; 0x20 + 6];
        e[8..16].copy_from_slice(&0u64.to_le_bytes());
        e[16..24].copy_from_slice(&5u64.to_le_bytes());
        e[28..32].copy_from_slice(&6u32.to_le_bytes());
        e[0x20..0x26].copy_from_slice(&[b'a', 0, b'b', 0, b'Z', 0]);
        t[0x20..0x20 + 0x26].copy_from_slice(&e[..0x26]);
        let mut blob = t;
        blob.extend_from_slice(b"HELLO");
        (blob, base)
    }

    fn case(tag: &str) -> (std::path::PathBuf, Vec<u8>, u64) {
        let (blob, base) = synth();
        let dir = std::env::temp_dir().join(format!("ie-romfs-{tag}-{}.d", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("r.bin");
        std::fs::write(&p, &blob).unwrap();
        (p, blob, base)
    }

    #[test]
    fn encuentra_descubre_y_resuelve() {
        let (p, blob, base) = case("a");
        let dir = p.parent().unwrap().to_path_buf();
        let cancel = AtomicBool::new(false);
        let tables = read_tables(&p, 0, blob.len() as u64).unwrap();
        let c = find_candidates(&tables, "abZ", 5);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].data_off, 0);
        let sha = sha256_range(&p, base, 5, &cancel, &mut |_| {}).unwrap();
        let ok =
            resolve_verified(&p, 0, blob.len() as u64, base, &c, &sha, "abZ", &cancel).unwrap();
        assert_eq!(ok.len(), 1);
        let disc =
            discover_file_data_base(&p, 0, blob.len() as u64, 5, &sha, &c, &cancel, &mut |_| {})
                .unwrap();
        assert_eq!(disc, base);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rebuild_apendiza_y_parchea() {
        let (p, blob, base) = case("b");
        let dir = p.parent().unwrap().to_path_buf();
        let n = dir.join("nuevo.bin");
        std::fs::write(&n, b"BYEBYE!").unwrap();
        let cancel = AtomicBool::new(false);
        let tables = read_tables(&p, 0, blob.len() as u64).unwrap();
        let loc = find_candidates(&tables, "abZ", 5).pop().unwrap();
        let r = Replacement {
            locs: vec![loc],
            new_file: n,
            new_len: 7,
        };
        let o = dir.join("o.bin");
        let layout = rebuild_append_only(
            &p,
            0,
            blob.len() as u64,
            base,
            &[r],
            &o,
            &cancel,
            &mut |_, _| {},
        )
        .unwrap();
        assert_eq!(layout.len(), 1);
        let got = std::fs::read(&o).unwrap();
        // original intacto + apéndice alineado a 0x200
        assert_eq!(&got[base as usize..base as usize + 5], b"HELLO");
        let at = (blob.len() as u64).next_multiple_of(0x200);
        assert_eq!(&got[at as usize..at as usize + 7], b"BYEBYE!");
        // entrada parcheada: doff = at - base, len 7
        assert_eq!(u64le(&got, 0x20 + 8), at - base);
        assert_eq!(u64le(&got, 0x20 + 16), 7);
        // layout devuelto coincide
        assert_eq!(layout[0].data_off, at - base);
        assert_eq!(layout[0].len, 7);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn sin_candidatos_es_error_claro() {
        let (p, blob, _) = case("c");
        let dir = p.parent().unwrap().to_path_buf();
        let tables = read_tables(&p, 0, blob.len() as u64).unwrap();
        assert!(find_candidates(&tables, "zz", 5).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }
}
