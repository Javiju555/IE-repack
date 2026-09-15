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
//! Estrategia de rebuild "solo-apéndice": los ficheros reemplazados se
//! escriben NUEVOS al final y sus entradas apuntan allí; el resto del blob
//! se copia tal cual. No se toca ninguna otra entrada (los alias por
//! deduplicación siguen válidos) a cambio de una imagen más grande.
//! Verificado byte a byte contra el prototipo con shifts (NOTA_IE123.md).
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
