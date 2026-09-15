// SPDX-FileCopyrightText: 2026 Javiju
//
// SPDX-License-Identifier: MIT

//! Integración real del modo manifiesto (ignorado en CI: necesita el dump
//! japonés y el pack de traducción, que nunca viajan en el repo).
//!
//!   IE123_ROM=/ruta/al/.3ds IE123_PACK=/ruta/paquete_v55 cargo test -p ie-core
//!   --test manifest_v55 -- --ignored --nocapture
//!
//! Verifica de forma independiente: TitleId, magia IVFC y los 200 SHA-256
//! resultado releídos de la imagen final.

use ie_core::{cci, manifest, ncch, pipeline, romfs};
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

#[test]
#[ignore]
fn v55_end_to_end() {
    let rom = PathBuf::from(std::env::var("IE123_ROM").expect("IE123_ROM"));
    let pack = PathBuf::from(std::env::var("IE123_PACK").expect("IE123_PACK"));
    let out = std::env::var("IE123_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("ie-v55-native.3ds"));
    let cancel = AtomicBool::new(false);

    pipeline::run_manifest(
        &pipeline::ManifestInputs {
            base: rom.clone(),
            pack_dir: pack.clone(),
            out_cci: out.clone(),
            keep_work: false,
        },
        &cancel,
        &mut |p| {
            if p.done == 0 || p.done == p.total {
                eprintln!("etapa {}/{}: {}", p.stage_idx + 1, p.stage_count, p.stage);
            }
        },
    )
    .expect("run_manifest");

    // Checks independientes del pipeline.
    let slots = cci::partitions(&out).unwrap();
    let (p0, _) = slots[0];
    let mut f = std::fs::File::open(&out).unwrap();
    f.seek(SeekFrom::Start(p0)).unwrap();
    let mut raw = [0u8; 0x200];
    f.read_exact(&mut raw).unwrap();
    let hdr = ncch::NcchHeader::parse(raw).unwrap();
    assert_eq!(&hdr.product_code(), "CTR-P-AETJ");
    let mut magic = [0u8; 4];
    f.seek(SeekFrom::Start(p0 + hdr.romfs_off)).unwrap();
    f.read_exact(&mut magic).unwrap();
    assert_eq!(&magic, b"IVFC");

    // Todos los SHAs resultado, releídos de la imagen final.
    let mani = manifest::load(&pack).unwrap();
    // FileData base: se re-deriva como hace el pipeline (tablas + menor).
    let romfs_abs = p0 + hdr.romfs_off;
    let tables = romfs::read_tables(&out, romfs_abs, hdr.romfs_size).unwrap();
    // Descubre B con el menor (igual que el pipeline).
    let small = mani.files.iter().min_by_key(|x| x.original_size).unwrap();
    let sn = small.ruta.rsplit('/').next().unwrap();
    // OJO: aquí se busca por tamaño RESULTADO (la imagen ya está parcheada).
    let sc = romfs::find_candidates(&tables, sn, small.resultado_size);
    let b = romfs::discover_file_data_base(
        &out,
        romfs_abs,
        hdr.romfs_size,
        small.resultado_size,
        &small.resultado_sha256.to_lowercase(),
        &sc,
        &cancel,
        &mut |_| {},
    )
    .unwrap();
    let mut bad = 0;
    for e in &mani.files {
        let name = e.ruta.rsplit('/').next().unwrap();
        let cs = romfs::find_candidates(&tables, name, e.resultado_size);
        let mut ok = false;
        for c in &cs {
            let abs = romfs_abs + b + c.data_off;
            if abs + c.len > romfs_abs + hdr.romfs_size {
                continue;
            }
            if romfs::sha256_range(&out, abs, c.len, &cancel, &mut |_| {}).unwrap()
                == e.resultado_sha256.to_lowercase()
            {
                ok = true;
                break;
            }
        }
        if !ok {
            bad += 1;
            eprintln!("FALTA EN FINAL: {}", e.ruta);
        }
    }
    assert_eq!(bad, 0, "ficheros sin verificar en la imagen final");
    eprintln!(
        "TODO VERIFICADO: {}/{} en la imagen final",
        mani.files.len(),
        mani.files.len()
    );
    // Se conserva la salida para inspección manual (boot en Azahar, etc.).
}
