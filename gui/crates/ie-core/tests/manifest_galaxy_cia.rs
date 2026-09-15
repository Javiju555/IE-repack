// SPDX-FileCopyrightText: 2026 Javiju
//
// SPDX-License-Identifier: MIT

//! Humo del modo manifiesto sobre CIA cifrado real (ignorado en CI:
//! necesita el CIA japonés, que nunca viaja en el repo).
//!
//!   GALAXY_CIA=/ruta/al.cia GALAXY_PACK=/ruta/minipack cargo test -p ie-core
//!   --test manifest_galaxy_cia -- --ignored --nocapture
//!
//! El minipack (1 fichero) se genera a mano: ver NOTA.
//! Verifica: TitleId Galaxy + SHA resultado releído de la imagen final.

use ie_core::{cci, manifest, ncch, pipeline, romfs};
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

#[test]
#[ignore]
fn galaxy_cia_minipack() {
    let cia = PathBuf::from(std::env::var("GALAXY_CIA").expect("GALAXY_CIA"));
    let pack = PathBuf::from(std::env::var("GALAXY_PACK").expect("GALAXY_PACK"));
    let out = std::env::var("GALAXY_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("ie-galaxy-minipack.3ds"));
    let cancel = AtomicBool::new(false);

    pipeline::run_manifest(
        &pipeline::ManifestInputs {
            base: cia.clone(),
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
    .expect("run_manifest galaxy cia");

    let slots = cci::partitions(&out).unwrap();
    assert_eq!(slots.len(), 2);
    let (p0, _) = slots[0];
    let mut f = std::fs::File::open(&out).unwrap();
    f.seek(SeekFrom::Start(p0)).unwrap();
    let mut raw = [0u8; 0x200];
    f.read_exact(&mut raw).unwrap();
    let hdr = ncch::NcchHeader::parse(raw).unwrap();
    assert!(hdr.product_code().contains("BGSJ"));

    let mani = manifest::load(&pack).unwrap();
    let romfs_abs = p0 + hdr.romfs_off;
    let tables = romfs::read_tables(&out, romfs_abs, hdr.romfs_size).unwrap();
    let small = &mani.files[0];
    let sn = small.ruta.rsplit('/').next().unwrap();
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
    let mut ok = false;
    for c in &sc {
        let abs = romfs_abs + b + c.data_off;
        if romfs::sha256_range(&out, abs, c.len, &cancel, &mut |_| {}).unwrap()
            == small.resultado_sha256.to_lowercase()
        {
            ok = true;
            break;
        }
    }
    assert!(ok, "resultado no encontrado en la imagen final");
    eprintln!("MINIPACK GALAXY CIA OK");
    std::fs::remove_file(&out).ok();
}
