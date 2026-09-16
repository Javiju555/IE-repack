// SPDX-FileCopyrightText: 2026 Javiju
//
// SPDX-License-Identifier: MIT

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
    /// Plantilla CIA (cabecera + offsets) para cuando la base es un volcado
    /// de tarjeta (CCI/`.3ds`): el xdelta espera bytes con envoltorio CIA y
    /// se reconstruyen desde el CCI normalizado. Vale cualquier CIA
    /// original del mismo juego y revisión (solo se copia su layout).
    pub cia_template: Option<PathBuf>,
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
/// Pública para reutilizar desde la GUI en el modo genérico.
pub fn extract_single_xdelta(zip_path: &Path, dest: &Path, cancel: &AtomicBool) -> Result<()> {
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
        // 0. parse-base: CIA directo, o CCI/.3ds + plantilla -> sintetiza CIA.
        emit(0, 0, 1);
        check_cancel(cancel)?;
        let _norm_guard: Option<crate::normalize::NormalizedCci>;
        let src_cia: PathBuf;
        if crate::cia::read_layout(&inp.base_cia).is_ok() {
            src_cia = inp.base_cia.clone();
            _norm_guard = None;
        } else {
            let norm = crate::normalize::normalize_to_decrypted_cci(
                &inp.base_cia, &work, cancel, &mut |_, _| {},
            )?;
            let template = inp.cia_template.as_ref().ok_or_else(|| {
                Error::Format(
                    "la base es un volcado de tarjeta (CCI/.3ds) y el parche espera un CIA: indica un CIA original del mismo juego como plantilla (solo se usa su layout)".into(),
                )
            })?;
            let slots = crate::cci::partitions_indexed(&norm.path)?;
            // Empareja por tamaño (el CCI trae particiones que el CIA no
            // lleva, como la de update): cada content de la plantilla coge
            // la partición que mida igual.
            let tlay = crate::cia::read_layout(template)?;
            let mut parts: Vec<crate::cia::SynthPart> = Vec::new();
            for c in tlay.contents.iter() {
                let hit = slots.iter().find(|(_, _, len)| *len == c.size).ok_or_else(|| {
                    Error::Format(format!(
                        "content{}: la plantilla espera {} bytes y ninguna partición de la base mide eso (¿otra revisión?)",
                        c.index, c.size
                    ))
                })?;
                parts.push(crate::cia::SynthPart { src: norm.path.clone(), src_off: hit.1, len: hit.2 });
            }
            let synth = work.join("synth.cia");
            crate::cia::synthesize_like(template, &parts, &synth, cancel, &mut |_, _| {})?;
            src_cia = synth;
            _norm_guard = Some(norm);
        }
        let layout = crate::cia::read_layout(&src_cia)?;
        let c0 = layout.content(0)?.clone();
        let _c1 = layout.content(1)?.clone();
        let base_hdr = read_ncch_header(&src_cia, c0.offset)?;
        emit(0, 1, 1);

        // Espacio libre orientativo. Pico real de temporal en disco: spliced
        // (se borra tras el decode) + forced + salida ≈ 3x la base. En RAM no
        // se cargan GBs en ningún momento: todo va por tramos de 64 MB.
        let need = layout.total_size.saturating_mul(4);
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
        copy_stream(&src_cia, &spliced, cancel, &mut |d| {
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
            extract_single_xdelta(&inp.patch, &dest, cancel)?;
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
        // El spliced ya no hace falta: se libera para bajar el pico en disco.
        if !inp.keep_work {
            std::fs::remove_file(&spliced).ok();
        }

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

        // 6. build-cci: lee los rangos directos del forzado (sin temporales
        //    de contents) y construye el .3ds por streaming.
        let fc1 = flayout.content(1)?.clone();
        let parts = [
            crate::cci::CciPart { path: &forced, offset: fc0.offset, len: fc0.size },
            crate::cci::CciPart { path: &forced, offset: fc1.offset, len: fc1.size },
        ];
        crate::cci::build_cci_from_parts(&parts, fhdr.title_id(), &inp.out_3ds, cancel, &mut |d, t| {
            emit(6, d, t);
        })?;
        crate::cci::verify_cci(&inp.out_3ds)?;
        emit(6, fc0.size + fc1.size, fc0.size + fc1.size);
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

// ---------------------------------------------------------------------------
// Modo manifiesto: pack de traducción por ficheros (manifiesto.json +
// parches .xdelta por fichero, p. ej. IE 1-2-3 ES v57).
//
// La base (CIA con 2 contenidos, o CCI con N particiones) se normaliza a CCI
// descifrado; cada fichero se localiza por verificación de contenido
// (SHA-256 original), se parchea en estricto, se verifica el resultado y se
// reconstruye el RomFS por apéndice. Todo verificado, nada forzado.

pub struct ManifestInputs {
    pub base: PathBuf,
    pub pack_dir: PathBuf,
    pub out_cci: PathBuf,
    pub keep_work: bool,
}

const MANIFEST_STAGES: [(&str, f32); 7] = [
    ("Leyendo base y manifiesto", 3.0),
    ("Normalizando (descifrado)", 18.0),
    ("Localizando ficheros", 9.0),
    ("Aplicando parches", 35.0),
    ("Reconstruyendo RomFS", 20.0),
    ("Ensamblando .3ds", 10.0),
    ("Verificando", 5.0),
];

/// Copia el rango [off, off+len) de `src` a `dst` por tramos.
fn copy_range_stream(
    src: &Path,
    off: u64,
    len: u64,
    dst: &mut std::fs::File,
    cancel: &AtomicBool,
    mut on_chunk: impl FnMut(u64),
) -> Result<()> {
    let mut f = std::fs::File::open(src)?;
    f.seek(SeekFrom::Start(off))?;
    let mut buf = vec![0u8; 8 * 1024 * 1024];
    let mut left = len;
    while left > 0 {
        check_cancel(cancel)?;
        let n = left.min(buf.len() as u64) as usize;
        f.read_exact(&mut buf[..n])?;
        dst.write_all(&mut buf[..n])?;
        left -= n as u64;
        on_chunk(n as u64);
    }
    Ok(())
}

pub fn run_manifest(
    inp: &ManifestInputs,
    cancel: &AtomicBool,
    prog: &mut dyn FnMut(StageProgress),
) -> Result<()> {
    let n_stages = MANIFEST_STAGES.len();
    fn mbase(idx: usize) -> f32 {
        MANIFEST_STAGES[..idx].iter().map(|(_, w)| w).sum()
    }
    let mut emit = |idx: usize, done: u64, total: u64| {
        let frac = if total == 0 { 1.0 } else { (done as f32 / total as f32).clamp(0.0, 1.0) };
        prog(StageProgress {
            stage_idx: idx,
            stage_count: n_stages,
            stage: MANIFEST_STAGES[idx].0,
            done,
            total,
            overall: ((mbase(idx) + frac * MANIFEST_STAGES[idx].1) / 100.0).clamp(0.0, 1.0),
        });
    };

    if !inp.base.is_file() {
        return Err(Error::Format("no existe el archivo base".into()));
    }
    if let Some(p) = inp.out_cci.parent() {
        if !p.as_os_str().is_empty() {
            std::fs::create_dir_all(p)?;
        }
    }
    let work = work_dir_for(&inp.out_cci)?;
    std::fs::create_dir_all(&work)?;
    let cleanup = |keep: bool| {
        if !keep {
            std::fs::remove_dir_all(&work).ok();
        }
    };

    let result = (|| -> Result<()> {
        // 0. Base + manifiesto.
        emit(0, 0, 3);
        check_cancel(cancel)?;
        let kind = crate::normalize::detect(&inp.base)?;
        // El normalizador acepta CIA con N contenidos (descifra todos y los
        // empaqueta en orden en los slots 0..N-1) y CCI con N particiones.
        // El juego vive en el content0/partición 0 en ambos casos.
        if kind == crate::normalize::InputKind::Cia {
            let layout = crate::cia::read_layout(&inp.base)?;
            if layout.contents.is_empty() {
                return Err(Error::Format("el CIA no trae contenidos".into()));
            }
        }
        let mani = {
            // El pack puede ser carpeta o .zip (se extrae a temporales).
            let dir = crate::manifest::prepare_pack(&inp.pack_dir, &work, cancel)?;
            crate::manifest::load(&dir)?
        };
        emit(0, 1, 3);
        let norm_len = std::fs::metadata(&inp.base)?.len();
        if let Ok(space) = fs2::available_space(work.clone()) {
            if space < norm_len.saturating_mul(6) {
                return Err(Error::Format("poco espacio donde va la salida (hacen falta ~12 GB libres)".into()));
            }
        }
        emit(0, 3, 3);

        // 1. Normalizar a CCI descifrado canónico (conserva N particiones).
        let norm = crate::normalize::normalize_to_decrypted_cci(&inp.base, &work, cancel, &mut |d, t| {
            emit(1, d, t);
        })?;

        // Rango del RomFS de la partición 0 en el normalizado.
        let slots = crate::cci::partitions_indexed(&norm.path)?;
        let (p0_idx, p0_off, p0_len) = *slots.first().ok_or_else(|| Error::Format("CCI sin particiones".into()))?;
        let _ = p0_idx;
        let hdr = read_ncch_header(&norm.path, p0_off)?;
        let romfs_abs = p0_off + hdr.romfs_off;
        let romfs_len = hdr.romfs_size;
        if romfs_len < 0x200 {
            return Err(Error::Format("RomFS sospechosamente pequeño (¿base válida?)".into()));
        }

        // 2. Tablas + descubrimiento de base + resolución verificada.
        let tables = crate::romfs::read_tables(&norm.path, romfs_abs, romfs_len)?;
        // Fichero más pequeño del manifiesto para descubrir B rápido.
        let small = mani.files.iter().min_by_key(|f| f.original_size).ok_or_else(|| Error::Format("manifiesto vacío".into()))?;
        let small_name = small.ruta.rsplit('/').next().unwrap_or(&small.ruta);
        let small_cands = crate::romfs::find_candidates(&tables, small_name, small.original_size);
        let total_probe = 0x400000u64;
        let file_data_base = crate::romfs::discover_file_data_base(
            &norm.path, romfs_abs, romfs_len,
            small.original_size, &small.original_sha256.to_lowercase(), &small_cands, cancel,
            &mut |b| emit(2, b / 16, total_probe / 16),
        )?;
        // Resuelve cada fichero (puerta estricta por contenido).
        struct Found {
            ruta: String,
            locs: Vec<crate::romfs::Located>,
            mf: crate::manifest::ManifestFile,
        }
        let mut found = Vec::with_capacity(mani.files.len());
        for (i, f) in mani.files.iter().enumerate() {
            check_cancel(cancel)?;
            emit(2, (i + 1) as u64, (mani.files.len() + 1) as u64);
            let name = f.ruta.rsplit('/').next().unwrap_or(&f.ruta);
            let cands = crate::romfs::find_candidates(&tables, name, f.original_size);
            let locs = crate::romfs::resolve_verified(
                &norm.path, romfs_abs, romfs_len, file_data_base,
                &cands, &f.original_sha256.to_lowercase(), &f.ruta, cancel,
            )?;
            found.push(Found { ruta: f.ruta.clone(), locs, mf: f.clone() });
        }
        emit(2, (mani.files.len() + 1) as u64, (mani.files.len() + 1) as u64);
        // Desempata por directorio: ficheros duplicados en varias carpetas
        // (mismos bytes, p. ej. inazuma1 vs inazuma2) verifican en varios
        // doffs. Los ficheros puros (un solo doff) votan el parent de cada
        // carpeta; los ambiguos eligen el doff con parent ganador.
        {
            use std::collections::{BTreeMap, BTreeSet};
            let dir_of = |ruta: &str| ruta.rsplit_once('/').map(|(d, _)| d.to_owned()).unwrap_or_default();
            let mut votes: BTreeMap<String, BTreeMap<u32, usize>> = BTreeMap::new();
            let doffs_of = |locs: &[crate::romfs::Located]| -> BTreeSet<u64> {
                locs.iter().map(|l| l.data_off).collect()
            };
            for f in &found {
                if doffs_of(&f.locs).len() == 1 {
                    for l in &f.locs {
                        *votes.entry(dir_of(&f.ruta)).or_default().entry(l.parent).or_insert(0) += 1;
                    }
                }
            }
            let mut mode: BTreeMap<String, u32> = BTreeMap::new();
            for (d, vs) in &votes {
                if let Some((p, _)) = vs.iter().max_by_key(|(_, n)| *n) {
                    mode.insert(d.clone(), *p);
                }
            }
            for f in &mut found {
                let mut doffs = doffs_of(&f.locs);
                if doffs.len() <= 1 {
                    continue;
                }
                let want = mode.get(&dir_of(&f.ruta)).copied().ok_or_else(|| {
                    Error::Verify(format!("{}: ambiguo y su carpeta no tiene mayoría (pack no soportado)", f.ruta))
                })?;
                let keep: Vec<u64> = f.locs.iter().filter(|l| l.parent == want).map(|l| l.data_off).collect();
                let keep_set: BTreeSet<u64> = keep.into_iter().collect();
                if keep_set.len() != 1 {
                    return Err(Error::Verify(format!(
                        "{}: ambiguo incluso dentro de su carpeta (pack no soportado)",
                        f.ruta
                    )));
                }
                let kd = *keep_set.iter().next().unwrap();
                f.locs.retain(|l| l.data_off == kd);
                doffs = doffs_of(&f.locs);
                let _ = doffs;
            }
        }

        // 3. Aplica cada xdelta en estricto + puerta de resultado.
        let xb = crate::xdelta::find_binary()?;
        let total_orig: u64 = mani.files.iter().map(|f| f.original_size).sum();
        let mut done_o = 0u64;
        let mut repl: Vec<crate::romfs::Replacement> = Vec::with_capacity(found.len());
        for (i, f) in found.iter().enumerate() {
            check_cancel(cancel)?;
            let abs0 = romfs_abs + file_data_base + f.locs[0].data_off;
            let orig_tmp = work.join(format!("orig{i}.bin"));
            let res_tmp = work.join(format!("res{i}.bin"));
            {
                let mut o = std::fs::File::create(&orig_tmp)?;
                copy_range_stream(&norm.path, abs0, f.mf.original_size, &mut o, cancel, &mut |_| {})?;
                o.flush()?;
            }
            let patch = mani.patch_path(&f.mf);
            crate::xdelta::decode_strict(&xb, &orig_tmp, &patch, &res_tmp, f.mf.resultado_size, cancel, &mut |_, _| {})?;
            std::fs::remove_file(&orig_tmp).ok();
            let got = crate::normalize::sha256_file(&res_tmp, cancel, &mut |_| {})?;
            if got != f.mf.resultado_sha256.to_lowercase() {
                return Err(Error::Verify(format!("{}: el parche no da lo esperado", f.ruta)));
            }
            done_o += f.mf.original_size;
            emit(3, done_o, total_orig);
            repl.push(crate::romfs::Replacement {
                locs: f.locs.clone(),
                new_file: res_tmp,
                new_len: f.mf.resultado_size,
            });
        }

        // 4. Reconstruye el blob RomFS (apéndice) en temporal.
        let new_blob = work.join("romfs_new.bin");
        let full = crate::romfs::rebuild_full(
            &norm.path, romfs_abs, romfs_len, file_data_base, &repl, &new_blob, cancel,
            &mut |d, t| emit(4, d, t),
        )?;
        let layout = full.locs;
        let new_hash_romfs = full.hash_romfs;
        let new_roh_units = full.romfs_hash_units;
        let want_locs: usize = repl.iter().map(|r| r.locs.len()).sum();
        if layout.len() != want_locs {
            return Err(Error::Format("layout reconstruido incompleto (bug interno)".into()));
        }
        for r in &repl {
            std::fs::remove_file(&r.new_file).ok();
        }
        let new_blob_len = std::fs::metadata(&new_blob)?.len();
        if new_blob_len % crate::cci::MEDIA_UNIT != 0 {
            return Err(Error::Format("RomFS reconstruido no alineado (bug interno)".into()));
        }

        // 5. Ensambla el CCI final en temporal: copia + romfs nuevo + cola
        //    desplazada + tabla NCSD + tamaño en NCCH.
        let tmp_out = work.join("out.3ds");
        let old_total = std::fs::metadata(&norm.path)?.len();
        let old_tail_off = romfs_abs + romfs_len;
        let delta = new_blob_len as i64 - romfs_len as i64;
        let new_total = (old_total as i64 + delta) as u64;
        {
            let mut o = std::fs::File::create(&tmp_out)?;
            // [0, romfs_abs): cabecera NCSD + partición hasta RomFS.
            {
                let mut f = std::fs::File::open(&norm.path)?;
                f.seek(SeekFrom::Start(0))?;
                let mut buf = vec![0u8; 8 * 1024 * 1024];
                let mut left = romfs_abs;
                let mut done = 0u64;
                while left > 0 {
                    check_cancel(cancel)?;
                    let n = left.min(buf.len() as u64) as usize;
                    f.read_exact(&mut buf[..n])?;
                    o.write_all(&buf[..n])?;
                    left -= n as u64;
                    done += n as u64;
                    emit(5, done, new_total);
                }
            }
            // RomFS nuevo.
            {
                let mut f = std::fs::File::open(&new_blob)?;
                let mut buf = vec![0u8; 8 * 1024 * 1024];
                let mut left = new_blob_len;
                let mut done = romfs_abs;
                loop {
                    check_cancel(cancel)?;
                    let n = f.read(&mut buf)?;
                    if n == 0 {
                        break;
                    }
                    o.write_all(&buf[..n])?;
                    left -= n as u64;
                    done += n as u64;
                    emit(5, done, new_total);
                }
                let _ = left;
            }
            // Cola desplazada.
            {
                let mut f = std::fs::File::open(&norm.path)?;
                f.seek(SeekFrom::Start(old_tail_off))?;
                let mut buf = vec![0u8; 8 * 1024 * 1024];
                let mut left = old_total - old_tail_off;
                let mut done = romfs_abs + new_blob_len;
                while left > 0 {
                    check_cancel(cancel)?;
                    let n = left.min(buf.len() as u64) as usize;
                    f.read_exact(&mut buf[..n])?;
                    o.write_all(&buf[..n])?;
                    left -= n as u64;
                    done += n as u64;
                    emit(5, done, new_total);
                }
            }
            o.flush()?;
        }
        // Parchea NCCH (romfs_size) y NCSD (slots + tarjeta).
        {
            let mut f = std::fs::OpenOptions::new().read(true).write(true).open(&tmp_out)?;
            let mut nh = [0u8; 0x200];
            f.seek(SeekFrom::Start(p0_off))?;
            f.read_exact(&mut nh)?;
            let media = 1u64 << (nh[0x18E] + 9);
            if new_blob_len % media != 0 {
                return Err(Error::Format("RomFS nuevo no alineado a media (bug interno)".into()));
            }
            nh[0x1B4..0x1B8].copy_from_slice(&((new_blob_len / media) as u32).to_le_bytes());
            // Hash base del RomFS (superblock+master nuevos) y su cobertura.
            nh[0x1B8..0x1BC].copy_from_slice(&new_roh_units.to_le_bytes());
            nh[0x1E0..0x200].copy_from_slice(&new_hash_romfs);
            // Content size = nueva partición 0. GodMode9 y el FS de la
            // consola lo usan para delimitar; si se queda viejo, el final
            // (justo donde van los ficheros nuevos) queda fuera.
            // El rebuild es COMPLETO con rehash IVFC (romfs.rs): tamaños y
            // cadena lvl2->lvl1->master coherentes, lo que verifica a fondo
            // GodMode9 (ver NOTA_IE123.md).
            let new_p0 = p0_len as i64 + delta;
            if new_p0 % media as i64 != 0 {
                return Err(Error::Format("partición nueva no alineada a media (bug interno)".into()));
            }
            nh[0x104..0x108].copy_from_slice(&((new_p0 / media as i64) as u32).to_le_bytes());
            f.seek(SeekFrom::Start(p0_off))?;
            f.write_all(&nh)?;
            // Slots: p0 crece; los posteriores se desplazan.
            let mut ncsd = [0u8; 0x200];
            f.seek(SeekFrom::Start(0))?;
            f.read_exact(&mut ncsd)?;
            let get = |o: usize| u32::from_le_bytes(ncsd[o..o + 4].try_into().unwrap()) as u64;
            let mut new_slots = [(0u64, 0u64); 8];
            for (i, slot) in new_slots.iter_mut().enumerate() {
                let (off, size) = (get(0x120 + i * 8) * crate::cci::MEDIA_UNIT, get(0x124 + i * 8) * crate::cci::MEDIA_UNIT);
                if size == 0 {
                    continue;
                }
                *slot = if off == p0_off {
                    (off / crate::cci::MEDIA_UNIT, (p0_len as i64 + delta) as u64 / crate::cci::MEDIA_UNIT)
                } else if off > p0_off {
                    ((off as i64 + delta) as u64 / crate::cci::MEDIA_UNIT, size / crate::cci::MEDIA_UNIT)
                } else {
                    (off / crate::cci::MEDIA_UNIT, size / crate::cci::MEDIA_UNIT)
                };
            }
            let media_size = crate::cci::card_media_size(new_total)?;
            // Blindaje: se conserva la cabecera NCSD original y solo se
            // parchean slots y tarjeta. Regenerarla de cero tumbó una vez el
            // loader de Azahar (fallo transitorio no reproducido, pero mejor
            // no tentar a la suerte con bytes de fabricante).
            let mut oh = [0u8; 0x200];
            {
                let mut nf = std::fs::File::open(&norm.path)?;
                nf.seek(SeekFrom::Start(0))?;
                nf.read_exact(&mut oh)?;
            }
            if &oh[0x100..0x104] != b"NCSD" {
                return Err(Error::Format("la base perdió la magia NCSD (bug interno)".into()));
            }
            let mut out_hdr = oh;
            out_hdr[0x104..0x108].copy_from_slice(&media_size.to_le_bytes());
            for i in 0..8 {
                let (off, size) = new_slots[i];
                out_hdr[0x120 + i * 8..0x124 + i * 8].copy_from_slice(&(off as u32).to_le_bytes());
                out_hdr[0x124 + i * 8..0x128 + i * 8].copy_from_slice(&(size as u32).to_le_bytes());
            }
            f.seek(SeekFrom::Start(0))?;
            f.write_all(&out_hdr)?;
            f.flush()?;
        }

        // 6. Verifica: TitleId + IVFC + SHAs resultado desde la imagen final.
        emit(6, 0, 3);
        check_cancel(cancel)?;
        let parts = crate::cci::partitions(&tmp_out)?;
        let (np0, _) = *parts.first().ok_or_else(|| Error::Format("salida sin particiones".into()))?;
        let nhdr = read_ncch_header(&tmp_out, np0)?;
        if nhdr.title_id() != norm.title_id {
            return Err(Error::Verify("el TitleId del resultado no coincide con la base".into()));
        }
        emit(6, 1, 4);
        if romfs_magic(&tmp_out, np0, &nhdr)? != *b"IVFC" {
            return Err(Error::Verify("el RomFS resultante no tiene magia IVFC".into()));
        }
        // Los SHAs resultado se releen de la imagen final (no de temporales).
        let nromfs_abs = np0 + nhdr.romfs_off;
        let total_res: u64 = mani.files.iter().map(|f| f.resultado_size).sum();
        let mut done_r = 0u64;
        // Las tablas se releen una vez (valen para todos los ficheros).
        let tabs = crate::romfs::read_tables(&tmp_out, nromfs_abs, nhdr.romfs_size)?;
        for f in mani.files.iter() {
            check_cancel(cancel)?;
            let want = f.resultado_sha256.to_lowercase();
            // Re-deriva: entradas de este fichero (por tamaño resultado).
            // (file_data_base no cambia con apéndice: la cabecera mide igual).
            let name = f.ruta.rsplit('/').next().unwrap_or(&f.ruta);
            let cands = crate::romfs::find_candidates(&tabs, name, f.resultado_size);
            let mut ok_one = false;
            for c in &cands {
                // file_data_base no cambia con apéndice (cabecera igual).
                let abs = nromfs_abs + file_data_base + c.data_off;
                if abs + c.len > nromfs_abs + nhdr.romfs_size {
                    continue;
                }
                let got = crate::romfs::sha256_range(&tmp_out, abs, c.len, cancel, &mut |_| {})?;
                if got == want {
                    ok_one = true;
                    break;
                }
            }
            if !ok_one {
                return Err(Error::Verify(format!("{}: no se encuentra el resultado en la imagen final", f.ruta)));
            }
            done_r += f.resultado_size;
            emit(6, 1 + 3 * done_r / total_res.max(1), 4);
        }
        crate::cci::verify_cci(&tmp_out)?;
        if !inp.keep_work {
            std::fs::remove_file(&new_blob).ok();
            std::fs::remove_file(&norm.path).ok();
        }

        if inp.out_cci.exists() {
            std::fs::remove_file(&inp.out_cci)?;
        }
        std::fs::rename(&tmp_out, &inp.out_cci)?;
        emit(6, 4, 4);
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

// ---------------------------------------------------------------------------
// Modo estricto genérico: base CIA/CCI -> CCI descifrado canónico -> cadena
// de parches aplicados CON checksums -> hash final opcional.
//
// A diferencia del modo Galaxy (forzado `-n`, necesario por el wrapper CIA
// único por dump), aquí un fallo de checksum significa base equivocada y debe
// abortar, no forzar: el resultado sería una quimera.

pub struct StrictInputs {
    pub base: PathBuf,
    /// Parches `.xdelta` en orden de aplicación.
    pub patches: Vec<PathBuf>,
    /// SHA-256 hex del CCI final completo (solo cuadra si la base era un
    /// volcado cartucho: el wrapper NCSD depende del linaje del dump).
    pub expected_sha256: Option<String>,
    /// SHA-256 hex del contenido (desde 0x200, sin cabecera NCSD). Es la
    /// puerta que sí puede pasar una base convertida desde CIA. Un proyecto
    /// la publica con: `tail -c +513 resultado.3ds | sha256sum`.
    pub expected_content_sha256: Option<String>,
    pub out_cci: PathBuf,
    pub keep_work: bool,
}

const STRICT_STAGES: [(&str, f32); 6] = [
    ("Leyendo base", 3.0),
    ("Normalizando (descifrado)", 30.0),
    ("Analizando parches", 4.0),
    ("Aplicando parches", 48.0),
    ("Verificando", 5.0),
    ("Guardando", 10.0),
];

pub fn run_strict(inp: &StrictInputs, cancel: &AtomicBool, prog: &mut dyn FnMut(StageProgress)) -> Result<()> {
    let n_stages = STRICT_STAGES.len();
    fn sbase(idx: usize) -> f32 {
        STRICT_STAGES[..idx].iter().map(|(_, w)| w).sum()
    }
    let mut emit = |idx: usize, done: u64, total: u64| {
        let frac = if total == 0 { 1.0 } else { (done as f32 / total as f32).clamp(0.0, 1.0) };
        prog(StageProgress {
            stage_idx: idx,
            stage_count: n_stages,
            stage: STRICT_STAGES[idx].0,
            done,
            total,
            overall: ((sbase(idx) + frac * STRICT_STAGES[idx].1) / 100.0).clamp(0.0, 1.0),
        });
    };

    if !inp.base.is_file() {
        return Err(Error::Format("no existe el archivo base".into()));
    }
    if inp.patches.is_empty() {
        return Err(Error::Format("no hay parches que aplicar".into()));
    }
    for p in &inp.patches {
        if !p.is_file() {
            return Err(Error::Format(format!("no existe el parche {}", p.display())));
        }
    }
    if let Some(p) = inp.out_cci.parent() {
        if !p.as_os_str().is_empty() {
            std::fs::create_dir_all(p)?;
        }
    }
    let work = work_dir_for(&inp.out_cci)?;
    std::fs::create_dir_all(&work)?;
    let cleanup = |keep: bool| {
        if !keep {
            std::fs::remove_dir_all(&work).ok();
        }
    };

    let result = (|| -> Result<()> {
        // 0. Tipo de base (falla pronto si no es CIA/CCI).
        emit(0, 0, 1);
        check_cancel(cancel)?;
        let kind = crate::normalize::detect(&inp.base)?;
        let _ = kind;
        emit(0, 1, 1);

        // Espacio orientativo: copia + forz... copia + final + salida.
        if let Ok(space) = fs2::available_space(work.clone()) {
            let need = std::fs::metadata(&inp.base)?.len().saturating_mul(4);
            if space < need {
                return Err(Error::Format(format!(
                    "poco espacio donde va la salida (hacen falta ~{} GB)",
                    need / 1_000_000_000
                )));
            }
        }

        // 1. Normalizar a CCI descifrado canónico.
        let norm = crate::normalize::normalize_to_decrypted_cci(&inp.base, &work, cancel, &mut |d, t| {
            emit(1, d, t);
        })?;

        // 2. Tamaños objetivo de cada parche (también valida que son xdelta).
        let xb = crate::xdelta::find_binary()?;
        let mut targets = Vec::with_capacity(inp.patches.len());
        for (i, p) in inp.patches.iter().enumerate() {
            check_cancel(cancel)?;
            emit(2, i as u64, inp.patches.len() as u64);
            targets.push(crate::xdelta::target_size(&xb, p)?);
        }
        emit(2, inp.patches.len() as u64, inp.patches.len() as u64);

        // 3. Cadena estricta. El último paso decodifica a temporal y luego se
        //    renombra (atómico en el mismo disco): nunca queda un parcial en
        //    la ruta de salida.
        let total_target: u64 = targets.iter().sum();
        let mut done_chain = 0u64;
        let mut cur = norm.path;
        let mut step_files = Vec::new();
        for (i, (patch, target)) in inp.patches.iter().zip(targets.iter()).enumerate() {
            check_cancel(cancel)?;
            let dest = work.join(format!("step{i}.3ds"));
            let before = done_chain;
            crate::xdelta::decode_strict(&xb, &cur, patch, &dest, *target, cancel, &mut |d, t| {
                emit(3, before + d * target / total_target.max(1), total_target);
                let _ = t;
            })
            .map_err(|e| match e {
                Error::Xdelta(m) => Error::Xdelta(format!(
                    "parche {} de {} ({}): {m}",
                    i + 1,
                    inp.patches.len(),
                    patch.file_name().and_then(|s| s.to_str()).unwrap_or("?")
                )),
                other => other,
            })?;
            done_chain += target;
            emit(3, done_chain, total_target);
            if i > 0 {
                std::fs::remove_file(&cur).ok();
            }
            step_files.push(dest.clone());
            cur = dest;
        }
        let _ = step_files;

        // 4. Verificar: mismo TitleId que la base + magia IVFC + hash si se dio.
        emit(4, 0, 3);
        check_cancel(cancel)?;
        let parts = crate::cci::partitions(&cur)?;
        let (p0_off, _) = *parts.first().ok_or_else(|| Error::Format("resultado sin particiones".into()))?;
        let hdr = read_ncch_header(&cur, p0_off)?;
        if hdr.title_id() != norm.title_id {
            return Err(Error::Verify(
                "el TitleId del resultado no coincide con la base (¿parche de otro juego?)".into(),
            ));
        }
        emit(4, 1, 3);
        if romfs_magic(&cur, p0_off, &hdr)? != *b"IVFC" {
            return Err(Error::Verify("el RomFS resultante no tiene magia IVFC".into()));
        }
        emit(4, 2, 3);
        let norm_hex = |s: &Option<String>| {
            s.as_ref().map(|x| x.trim().to_lowercase()).filter(|x| !x.is_empty())
        };
        if let Some(want) = norm_hex(&inp.expected_sha256) {
            let got = crate::normalize::sha256_file(&cur, cancel, &mut |_| {})?;
            if got != want {
                return Err(Error::Verify(format!(
                    "SHA-256 final no coincide: esperado {want}, obtenido {got}. \
                     Si tu base viene de un CIA convertido (no de cartucho), la cabecera \
                     NCSD difiere por linaje: usa el SHA-256 de contenido en su lugar."
                )));
            }
        }
        if let Some(want) = norm_hex(&inp.expected_content_sha256) {
            let got = crate::normalize::sha256_cci_content(&cur, cancel, &mut |_| {})?;
            if got != want {
                return Err(Error::Verify(format!(
                    "SHA-256 de contenido no coincide: esperado {want}, obtenido {got}"
                )));
            }
        }
        emit(4, 3, 3);

        // 5. Guardar (rename atómico; el temporal vive junto a la salida).
        if inp.out_cci.exists() {
            std::fs::remove_file(&inp.out_cci)?;
        }
        std::fs::rename(&cur, &inp.out_cci)?;
        emit(5, 1, 1);
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
