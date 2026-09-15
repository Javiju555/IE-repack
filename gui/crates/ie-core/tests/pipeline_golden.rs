// SPDX-FileCopyrightText: 2026 Javiju
//
// SPDX-License-Identifier: MIT

//! Test dorado end-to-end (LOCAL, ignorado por defecto).
//!
//! Ejecuta el pipeline nativo completo sobre el dump propio y comprueba que
//! el `.3ds` resultante lleva el contenido traducido de referencia:
//! cabecera NCCH idéntica al CIA ESP de referencia + SHA256 de `ie6_a.fa` /
//! `ie6_b.fa` coincidentes.
//!
//! Requiere (nunca en el repo):
//!   IE_GOLDEN_BASE  = CIA japonés propio
//!   IE_GOLDEN_PATCH = parche .xdelta (o .zip) público
//!   IE_GOLDEN_ESP   = CIA ya traducido de referencia
//!   IE_GOLDEN_OUT   = directorio en disco real con ~25 GB libres
//!   xdelta3 en PATH o IE_XDELTA3
//!
//! Se ejecuta con: cargo test -p ie-core --test pipeline_golden -- --ignored

use ie_core::{cci, cia, pipeline};
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

const REF_A: &str = "cda8989e65c24d27f73b9832bce8531d654ef1783a091053d673022f2099c78f";
const REF_B: &str = "1c291c2564bf85b3f88a8c16f4fc1727983d2f447fdb427f38adb28d2e20418c";

fn sha_range(path: &Path, off: u64, len: u64) -> String {
    let mut f = std::fs::File::open(path).unwrap();
    f.seek(SeekFrom::Start(off)).unwrap();
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024 * 1024];
    let mut left = len;
    while left > 0 {
        let n = left.min(buf.len() as u64) as usize;
        f.read_exact(&mut buf[..n]).unwrap();
        h.update(&buf[..n]);
        left -= n as u64;
    }
    format!("{:x}", h.finalize())
}

fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// Lee la cabecera NCCH (512 B) en `off` dentro de `path`.
fn ncch_raw(path: &Path, off: u64) -> [u8; 0x200] {
    let mut f = std::fs::File::open(path).unwrap();
    f.seek(SeekFrom::Start(off)).unwrap();
    let mut raw = [0u8; 0x200];
    f.read_exact(&mut raw).unwrap();
    raw
}

/// Parser mínimo de Level-3 RomFS: nombre -> (offset_absoluto, size).
fn romfs_files(path: &Path, romfs_abs: u64) -> Vec<(String, u64, u64)> {
    let mut f = std::fs::File::open(path).unwrap();
    let l3 = romfs_abs + 0x1000;
    f.seek(SeekFrom::Start(l3)).unwrap();
    let mut hdr = [0u8; 0x28];
    f.read_exact(&mut hdr).unwrap();
    assert_eq!(u32le(&hdr, 0), 0x28, "Level-3 inválido");
    let (fmo, fml, fdo) = (u32le(&hdr, 0x1C) as u64, u32le(&hdr, 0x20) as u64, u32le(&hdr, 0x24) as u64);
    f.seek(SeekFrom::Start(l3 + fmo)).unwrap();
    let mut meta = vec![0u8; fml as usize];
    f.read_exact(&mut meta).unwrap();
    let mut out = Vec::new();
    let mut o = 0usize;
    while o < meta.len() {
        let data_off = u64::from_le_bytes(meta[o + 8..o + 16].try_into().unwrap());
        let data_size = u64::from_le_bytes(meta[o + 16..o + 24].try_into().unwrap());
        let name_len = u32le(&meta, o + 28) as usize;
        let name = String::from_utf16_lossy(
            &meta[o + 32..o + 32 + name_len].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>(),
        );
        out.push((name, l3 + fdo + data_off, data_size));
        o += 32 + ((name_len + 3) / 4) * 4;
    }
    out
}

#[test]
#[ignore]
fn pipeline_produce_traduccion_de_referencia() {
    let base = PathBuf::from(std::env::var("IE_GOLDEN_BASE").expect("IE_GOLDEN_BASE"));
    let patch = PathBuf::from(std::env::var("IE_GOLDEN_PATCH").expect("IE_GOLDEN_PATCH"));
    let esp = PathBuf::from(std::env::var("IE_GOLDEN_ESP").expect("IE_GOLDEN_ESP"));
    let outdir = PathBuf::from(std::env::var("IE_GOLDEN_OUT").expect("IE_GOLDEN_OUT"));
    let out = outdir.join("golden.3ds");

    let cancel = AtomicBool::new(false);
    pipeline::run(
        &pipeline::Inputs { base_cia: base, patch, out_3ds: out.clone(), keep_work: false },
        &cancel,
        &mut |p| eprintln!("{}: {:.1}%", p.stage, p.overall * 100.0),
    )
    .unwrap();

    // Estructura válida (magia + hashes NCCH recomputados).
    cci::verify_cci(&out).unwrap();

    // Envoltura NCSD como makerom: firma presente (Azahar la exige aunque sea
    // relleno) y segundo id con tipo 0x0005. Previene volver a ceros.
    {
        use std::io::Read;
        let mut f = std::fs::File::open(&out).unwrap();
        let mut hdr = [0u8; 0x200];
        f.read_exact(&mut hdr).unwrap();
        const FILLER: &[u8; 256] = include_bytes!("../src/ncsd_rsa_filler.bin");
        assert!(FILLER.iter().any(|&b| b != 0));
        assert_eq!(&hdr[..0x100], FILLER, "firma NCSD ausente");
        assert_eq!(&hdr[0x198..0x1A0], &0x0005_0000_0010_BB00u64.to_le_bytes());
    }

    // Cabecera NCCH del content0 idéntica a la referencia ESP.
    let parts = cci::partitions(&out).unwrap();
    let got_hdr = ncch_raw(&out, parts[0].0);
    let esp_layout = cia::read_layout(&esp).unwrap();
    let esp_c0 = esp_layout.content(0).unwrap();
    let ref_hdr = ncch_raw(&esp, esp_c0.offset);
    assert_eq!(got_hdr, ref_hdr, "cabecera NCCH distinta de la referencia");

    // .fa idénticos a la referencia.
    let romfs_off =
        u32::from_le_bytes(got_hdr[0x1B0..0x1B4].try_into().unwrap()) as u64 * 0x200;
    let files = romfs_files(&out, parts[0].0 + romfs_off);
    for (want_name, want_sha) in [("ie6_a.fa", REF_A), ("ie6_b.fa", REF_B)] {
        let (_, off, size) =
            files.iter().find(|(n, _, _)| n == want_name).expect("falta .fa en el CCI");
        assert_eq!(sha_range(&out, *off, *size), want_sha, "{want_name} no coincide");
    }
}
