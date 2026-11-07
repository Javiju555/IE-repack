// SPDX-FileCopyrightText: 2026 Javiju
//
// SPDX-License-Identifier: MIT

//! Writer `.cia` contra dumps reales. Ignorado por defecto: necesita la base
//! japonesa y un CIA donante del mismo juego en disco.
//!
//! ```bash
//! IE_CIA_BASE="/ruta/base japonesa.3ds" \
//! IE_CIA_DONOR="/ruta/donante.cia" \
//! IE_CIA_OUT="/ruta/salida.cia" \
//!   cargo test -p ie-core --test cia_real -- --ignored --nocapture
//! ```
//!
//! Comprueba que la salida re-parsea, que cada content conserva tamaño y
//! SHA-256 del CCI de partida y que el TMD lleva el ticket falso y el
//! contentinfo rehasheado. No sustituye a instalarlo en una consola.

use ie_core::cia;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

fn read_at(path: &Path, off: u64, len: usize) -> Vec<u8> {
    let mut f = File::open(path).expect("abrir");
    f.seek(SeekFrom::Start(off)).expect("seek");
    let mut b = vec![0u8; len];
    f.read_exact(&mut b).expect("leer");
    b
}

fn sha_stream(path: &Path, off: u64, len: u64) -> [u8; 32] {
    let mut f = File::open(path).expect("abrir");
    f.seek(SeekFrom::Start(off)).expect("seek");
    let mut h = Sha256::new();
    let mut left = len;
    let mut buf = vec![0u8; 8 * 1024 * 1024];
    while left > 0 {
        let n = left.min(buf.len() as u64) as usize;
        f.read_exact(&mut buf[..n]).expect("leer");
        h.update(&buf[..n]);
        left -= n as u64;
    }
    h.finalize().into()
}

#[test]
#[ignore = "necesita dumps reales: IE_CIA_BASE + IE_CIA_DONOR"]
fn cia_real_desde_dumps() {
    let (Ok(base), Ok(donor)) = (std::env::var("IE_CIA_BASE"), std::env::var("IE_CIA_DONOR"))
    else {
        eprintln!("faltan IE_CIA_BASE / IE_CIA_DONOR: nada que probar");
        return;
    };
    let base = PathBuf::from(base);
    let donor = PathBuf::from(donor);
    let out = std::env::var_os("IE_CIA_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("ie-cia-real.cia"));
    std::fs::remove_file(&out).ok();

    let cancel = AtomicBool::new(false);
    let mut ultimo = 0u64;
    let t0 = std::time::Instant::now();
    cia::build_from_cci(&base, &donor, &out, &cancel, |d, t| {
        if d / (512 * 1024 * 1024) != ultimo / (512 * 1024 * 1024) {
            eprintln!("  {d} / {t} bytes");
            ultimo = d;
        }
    })
    .expect("build_from_cci falló");
    eprintln!("writer: {:.1} s", t0.elapsed().as_secs_f64());

    // La función ya re-parsea y compara SHAs; aquí se comprueba desde fuera.
    let layout = cia::read_layout(&out).expect("la salida no re-parsea");
    let dlayout = cia::read_layout(&donor).expect("el donante no re-parsea");
    eprintln!(
        "salida: {} bytes, {} contents",
        layout.total_size,
        layout.contents.len()
    );
    assert_eq!(layout.contents.len(), dlayout.contents.len());

    // Cabecera: ticket falso + TMD con la firma a 0xFF y su conteo.
    let hdr = read_at(&out, 0, 0x20);
    let cert_len = u32::from_le_bytes(hdr[0x08..0x0C].try_into().unwrap()) as u64;
    let tik_len = u32::from_le_bytes(hdr[0x0C..0x10].try_into().unwrap()) as u64;
    let tmd_len = u32::from_le_bytes(hdr[0x10..0x14].try_into().unwrap()) as u64;
    let tik = (0x2040 + cert_len).next_multiple_of(64);
    let tikb = read_at(&out, tik, 0x350);
    assert_eq!(&tikb[0..4], &0x10004u32.to_be_bytes(), "tipo de ticket");
    assert!(
        tikb[4..0x104].iter().all(|b| *b == 0xFF),
        "firma del ticket a 0xFF"
    );
    assert!(
        tikb[0x1BF..0x1CF].iter().all(|b| *b == 0xFF),
        "titlekey a 0xFF"
    );
    let tmd_off = (tik + tik_len).next_multiple_of(64);
    let tmd = read_at(&out, tmd_off, tmd_len as usize);
    let sig_type = u32::from_be_bytes(tmd[0..4].try_into().unwrap());
    let sig_len = match sig_type {
        0x10003 => 0x200usize,
        0x10004 => 0x100,
        0x10005 => 0x3C,
        t => panic!("firma de TMD inesperada: {t:#x}"),
    };
    let head = (4 + sig_len + 63) & !63;
    assert!(
        tmd[4..4 + sig_len].iter().all(|b| *b == 0xFF),
        "firma del TMD a 0xFF"
    );
    let count = u16::from_be_bytes(tmd[head + 0x9E..head + 0xA0].try_into().unwrap()) as usize;
    assert_eq!(count, layout.contents.len(), "records del TMD");

    // Cada content del CIA sale del slot homónimo del CCI: mismo tamaño y SHA.
    // El donante decide qué contents hay; el resto de slots del CCI (p. ej. el
    // manual del slot 7) se quedan fuera a propósito.
    let slots = ie_core::cci::partitions_indexed(&base).expect("CCI sin particiones");
    let fuera: Vec<usize> = slots
        .iter()
        .map(|(s, _, _)| *s)
        .filter(|s| !layout.contents.iter().any(|c| c.index as usize == *s))
        .collect();
    if !fuera.is_empty() {
        eprintln!("slots del CCI fuera del CIA (por el donante): {fuera:?}");
    }

    for c in &layout.contents {
        let (slot, off, size) = *slots
            .iter()
            .find(|(s, _, _)| *s as u16 == c.index)
            .unwrap_or_else(|| panic!("content{}: el CCI no trae ese slot", c.index));
        let nombre = format!("content{} / slot{slot}", c.index);
        assert_eq!(c.size, size, "{nombre}: tamaño distinto al CCI");
        let h = sha_stream(&out, c.offset, c.size);
        let g = sha_stream(&base, off, size);
        assert_eq!(h, g, "{nombre}: sha distinto al CCI");
        eprintln!("{nombre}: {size} bytes OK");
    }
    eprintln!("OK: contents y SHA-256 coinciden con el CCI");
    std::fs::remove_file(&out).ok();
}
