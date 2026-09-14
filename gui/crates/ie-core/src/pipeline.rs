//! Pipeline completo del modo `--xdelta-patch` con progreso por etapas.
//!
//! Etapas (pesos sobre 100 para la barra global):
//! 1. parse-base (2): lee el layout del CIA base.
//! 2. decrypt (33): copia la base y la descifra in place (mismo layout).
//! 3. patch (2): resuelve el `.xdelta` (o lo extrae del `.zip`).
//! 4. xinfo (1): tamaño objetivo vía `xdelta3 printhdrs`.
//! 5. decode (45): `xdelta3 -d -n` con progreso por crecimiento del fichero.
//! 6. verify (2): TitleId base == resultado + magia IVFC.
//! 7. build-cci (15): extrae contents del forzado y construye el `.3ds`.

use crate::error::{Error, Result};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone, Copy)]
pub struct StageProgress {
    pub stage_idx: usize,
    pub stage_count: usize,
    pub stage: &'static str,
    pub done: u64,
    pub total: u64,
    /// 0.0..=1.0 global.
    pub overall: f32,
}

pub struct Inputs {
    pub base_cia: PathBuf,
    pub patch: PathBuf,
    pub out_3ds: PathBuf,
    pub keep_work: bool,
}

const STAGES: [(&str, f32); 7] = [
    ("Leyendo el CIA base", 2.0),
    ("Descifrando la base", 33.0),
    ("Preparando el parche", 2.0),
    ("Analizando el parche", 1.0),
    ("Aplicando la traducción", 45.0),
    ("Verificando el resultado", 2.0),
    ("Construyendo el .3ds", 15.0),
];

fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        return Err(Error::Cancelled);
    }
    Ok(())
}

fn copy_stream(
    src: &Path,
    dst: &Path,
    cancel: &AtomicBool,
    mut on_chunk: impl FnMut(u64),
) -> Result<u64> {
    let mut f = std::fs::File::open(src)?;
    let mut o = std::fs::File::create(dst)?;
    let mut buf = vec![0u8; 64 * 1024 * 1024];
    let mut done = 0u64;
    loop {
        check_cancel(cancel)?;
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

fn copy_range(
    src: &Path,
    off: u64,
    len: u64,
    dst: &Path,
    cancel: &AtomicBool,
    mut on_chunk: impl FnMut(u64),
) -> Result<()> {
    let mut f = std::fs::File::open(src)?;
    let mut o = std::fs::File::create(dst)?;
    f.seek(SeekFrom::Start(off))?;
    let mut buf = vec![0u8; 64 * 1024 * 1024];
    let mut left = len;
    while left > 0 {
        check_cancel(cancel)?;
        let n = left.min(buf.len() as u64) as usize;
        f.read_exact(&mut buf[..n])?;
        o.write_all(&mut buf[..n])?;
        left -= n as u64;
        on_chunk(n as u64);
    }
    o.flush()?;
    Ok(())
}

fn work_dir_for(out_3ds: &Path) -> Result<PathBuf> {
    let parent = out_3ds.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    Ok(parent.join(format!(".ie-work-{pid}-{nanos:x}")))
}

/// Extrae el único `.xdelta` del `.zip` (misma regla que rebuild.sh).
fn extract_xdelta_from_zip(zip_path: &Path, dest: &Path, cancel: &AtomicBool) -> Result<()> {
    let f = std::fs::File::open(zip_path)?;
    let mut zip = zip::ZipArchive::new(f).map_err(|e| Error::Zip(format!("no es un .zip válido: {e}")))?;
    let mut found = None;
    for i in 0..zip.len() {
        let e = zip.by_index(i).map_err(|e| Error::Zip(format!("{e}")))?;
        if e.name().to_lowercase().ends_with(".xdelta") {
            if found.is_some() {
                return Err(Error::Zip(
                    "el .zip trae varios .xdelta: extrae a mano el que corresponda".into(),
                ));
            }
            found = Some(i);
        }
    }
    let idx = found.ok_or_else(|| Error::Zip("el .zip no contiene ningún .xdelta".into()))?;
    let mut e = zip.by_index(idx).map_err(|e| Error::Zip(format!("{e}")))?;
    let mut o = std::fs::File::create(dest)?;
    let mut buf = vec![0u8; 8 * 1024 * 1024];
    loop {
        check_cancel(cancel)?;
        let n = e.read(&mut buf).map_err(|e| Error::Zip(format!("{e}")))?;
        if n == 0 {
            break;
        }
        o.write_all(&buf[..n])?;
    }
    o.flush()?;
    Ok(())
}

fn read_ncch_header(path: &Path, c0_off: u64) -> Result<crate::ncch::NcchHeader> {
    let mut f = std::fs::File::open(path)?;
    f.seek(SeekFrom::Start(c0_off))?;
    let mut raw = [0u8; 0x200];
    f.read_exact(&mut raw)?;
    crate::ncch::NcchHeader::parse(raw)
}

fn romfs_magic(path: &Path, c0_off: u64, hdr: &crate::ncch::NcchHeader) -> Result<[u8; 4]> {
    let mut f = std::fs::File::open(path)?;
    f.seek(SeekFrom::Start(c0_off + hdr.romfs_off))?;
    let mut magic = [0u8; 4];
    f.read_exact(&mut magic)?;
    Ok(magic)
}

pub fn run(inp: &Inputs, cancel: &AtomicBool, prog: &mut dyn FnMut(StageProgress)) -> Result<()> {
    let n_stages = STAGES.len();
    // Suma de pesos hasta (sin incluir) idx.
    fn base_of(idx: usize) -> f32 {
        STAGES[..idx].iter().map(|(_, w)| w).sum()
    }
    let mut emit = |idx: usize, done: u64, total: u64| {
        let frac = if total == 0 { 1.0 } else { (done as f32 / total as f32).clamp(0.0, 1.0) };
        prog(StageProgress {
            stage_idx: idx,
            stage_count: n_stages,
            stage: STAGES[idx].0,
            done,
            total,
            overall: ((base_of(idx) + frac * STAGES[idx].1) / 100.0).clamp(0.0, 1.0),
        });
    };

    if !inp.base_cia.is_file() {
        return Err(Error::Format("no existe el CIA base".into()));
    }
    if !inp.patch.is_file() {
        return Err(Error::Format("no existe el parche".into()));
    }
    if let Some(p) = inp.out_3ds.parent() {
        if !p.as_os_str().is_empty() {
            std::fs::create_dir_all(p)?;
        }
    }
    let work = work_dir_for(&inp.out_3ds)?;
    std::fs::create_dir_all(&work)?;
    let cleanup = |keep: bool| {
        if !keep {
            std::fs::remove_dir_all(&work).ok();
        }
    };

    let result = (|| -> Result<()> {
        // 0. parse-base.
        emit(0, 0, 1);
        check_cancel(cancel)?;
        let layout = crate::cia::read_layout(&inp.base_cia)?;
        let c0 = layout.content(0)?.clone();
        let _c1 = layout.content(1)?.clone();
        let base_hdr = read_ncch_header(&inp.base_cia, c0.offset)?;
        emit(0, 1, 1);

        // Espacio libre orientativo (pico real ~4x la base: spliced + forced
        // + contents + salida).
        let need = layout.total_size.saturating_mul(5);
        if let Ok(space) = fs2::available_space(work.clone()) {
            if space < need {
                return Err(Error::Format(format!(
                    "poco espacio donde va la salida (hacen falta ~{} GB)",
                    need / 1_000_000_000
                )));
            }
        }

        // 1. decrypt: copia + in place.
        let spliced = work.join("spliced.cia");
        let total = layout.total_size;
        let mut done_c = 0u64;
        copy_stream(&inp.base_cia, &spliced, cancel, &mut |d| {
            done_c += d;
            emit(1, done_c / 2, total);
        })?;
        crate::ncch::decrypt_content_in_place(&spliced, c0.offset, cancel, &mut |d, t| {
            // Segunda mitad de la etapa.
            let _ = t;
            emit(1, total / 2 + d / 2, total);
        })?;

        // 2. patch.
        emit(2, 0, 1);
        check_cancel(cancel)?;
        let is_zip = inp.patch.extension().map(|e| e.eq_ignore_ascii_case("zip")).unwrap_or(false);
        let xdelta_file = if is_zip {
            let dest = work.join("patch.xdelta");
            extract_xdelta_from_zip(&inp.patch, &dest, cancel)?;
            dest
        } else {
            inp.patch.clone()
        };
        emit(2, 1, 1);

        // 3. xinfo.
        emit(3, 0, 1);
        check_cancel(cancel)?;
        let xb = crate::xdelta::find_binary()?;
        let expected = crate::xdelta::target_size(&xb, &xdelta_file)?;
        if expected < 1_000_000_000 || expected > 8_000_000_000 {
            return Err(Error::Verify(format!(
                "el parche declara un tamaño objetivo raro ({expected} bytes); ¿es el parche de este juego?"
            )));
        }
        emit(3, 1, 1);

        // 4. decode.
        let forced = work.join("forced.cia");
        crate::xdelta::decode_forced(&xb, &spliced, &xdelta_file, &forced, expected, cancel, &mut |d, t| {
            emit(4, d, t);
        })?;

        // 5. verify.
        emit(5, 0, 2);
        check_cancel(cancel)?;
        let flayout = crate::cia::read_layout(&forced)?;
        let fc0 = flayout.content(0)?.clone();
        let fhdr = read_ncch_header(&forced, fc0.offset)?;
        if fhdr.title_id() != base_hdr.title_id() {
            return Err(Error::Verify(
                "el TitleId del resultado no coincide con la base (¿parche de otro juego?)".into(),
            ));
        }
        emit(5, 1, 2);
        if romfs_magic(&forced, fc0.offset, &fhdr)? != *b"IVFC" {
            return Err(Error::Verify("el RomFS resultante no tiene magia IVFC".into()));
        }
        emit(5, 2, 2);

        // 6. build-cci: extracción 0..2/3, construcción 2/3..1.
        let fc1 = flayout.content(1)?.clone();
        let c0path = work.join("content0.cxi");
        let c1path = work.join("content1.bin");
        let mut done_b = 0u64;
        let grand_b = fc0.size + fc1.size;
        copy_range(&forced, fc0.offset, fc0.size, &c0path, cancel, &mut |d| {
            done_b += d;
            emit(6, done_b * 2 / 3, grand_b);
        })?;
        copy_range(&forced, fc1.offset, fc1.size, &c1path, cancel, &mut |d| {
            done_b += d;
            emit(6, done_b * 2 / 3, grand_b);
        })?;
        crate::cci::build_cci(&c0path, &c1path, fhdr.title_id(), &inp.out_3ds, cancel, &mut |d, t| {
            let frac = if t == 0 { 1.0 } else { (d as f32 / t as f32).clamp(0.0, 1.0) };
            emit(6, (grand_b as f32 * (2.0 + frac) / 3.0) as u64, grand_b);
        })?;
        crate::cci::verify_cci(&inp.out_3ds)?;
        emit(6, grand_b, grand_b);
        Ok(())
    })();

    match &result {
        Ok(()) => {
            cleanup(inp.keep_work);
            prog(StageProgress {
                stage_idx: n_stages - 1,
                stage_count: n_stages,
                stage: "Completado",
                done: 1,
                total: 1,
                overall: 1.0,
            });
        }
        Err(_) => {
            cleanup(false);
        }
    }
    result
}
