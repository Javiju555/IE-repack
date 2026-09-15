// SPDX-FileCopyrightText: 2026 Javiju
//
// SPDX-License-Identifier: MIT

//! Modo manifiesto sobre un CCI sintético mínimo (rápido, apto para CI con
//! xdelta3 en el PATH): 1 partición, RomFS con 2 ficheros, pack de 1
//! reemplazo con cambio de tamaño. Verifica ensamblaje, shifts y contenido.

use ie_core::{cci, manifest, pipeline};
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

fn sha(b: &[u8]) -> String {
    format!("{:x}", Sha256::digest(b))
}

/// Construye un CCI mínimo: NCSD(1 slot) + NCCH descifrado + ExeFS tonto +
/// RomFS con tablas [f1(len5,doff0), f2(len4,doff8)] + datos.
fn build_synth(dir: &Path) -> (PathBuf, u64, u64) {
    // --- RomFS blob ---
    // Entradas FUERA de la región hasheada [0, 0x200): como en los juegos
    // reales, donde el superblock cubierto por el hash no contiene entradas.
    let file_data_base = 0x300u64;
    let mut head = vec![0u8; file_data_base as usize];
    head[0..4].copy_from_slice(b"IVFC"); // el pipeline exige la magia
    // entrada f1 @0x210: doff 0, len 5, nombre "f1"
    head[0x210 + 8..0x210 + 16].copy_from_slice(&0u64.to_le_bytes());
    head[0x210 + 16..0x210 + 24].copy_from_slice(&5u64.to_le_bytes());
    head[0x210 + 28..0x210 + 32].copy_from_slice(&4u32.to_le_bytes());
    head[0x230..0x234].copy_from_slice(&[b'f', 0, b'1', 0]);
    // entrada f2 @0x250: doff 8, len 4, nombre "f2"
    head[0x250 + 8..0x250 + 16].copy_from_slice(&8u64.to_le_bytes());
    head[0x250 + 16..0x250 + 24].copy_from_slice(&4u64.to_le_bytes());
    head[0x250 + 28..0x250 + 32].copy_from_slice(&4u32.to_le_bytes());
    head[0x270..0x274].copy_from_slice(&[b'f', 0, b'2', 0]);
    let mut romfs = head;
    romfs.extend_from_slice(b"AAAAAZZZ1234"); // f1 @0, hueco, f2 @8
    while romfs.len() % 0x200 != 0 {
        romfs.push(0);
    }
    let romfs_len = romfs.len() as u64;

    // --- NCCH ---
    let exefs = vec![0u8; 0x200];
    let ncch_off = 0x4000u64;
    let exefs_off = 0x200u64; // relativo a partición
    let romfs_off = exefs_off + 0x200;
    let mut ncch = vec![0u8; 0x200];
    ncch[0x100..0x104].copy_from_slice(b"NCCH");
    ncch[0x108..0x110].copy_from_slice(&0x00040000000EDF00u64.to_le_bytes());
    ncch[0x150..0x15A].copy_from_slice(b"CTR-P-AETJ");
    ncch[0x18E] = 0; // media 0x200
    ncch[0x18F] = 0x04; // NoEncrypto -> descifrado
    ncch[0x1A0..0x1A4].copy_from_slice(&1u32.to_le_bytes()); // exefs 1 media
    ncch[0x1A4..0x1A8].copy_from_slice(&1u32.to_le_bytes());
    ncch[0x1A8..0x1AC].copy_from_slice(&1u32.to_le_bytes()); // hash exefs: 0x200
    ncch[0x1B0..0x1B4].copy_from_slice(&((romfs_off / 0x200) as u32).to_le_bytes());
    ncch[0x1B4..0x1B8].copy_from_slice(&((romfs_len / 0x200) as u32).to_le_bytes());
    ncch[0x1B8..0x1BC].copy_from_slice(&1u32.to_le_bytes()); // hash romfs: 0x200 (sintético)
    assert_eq!(romfs_len % 0x200, 0, "el blob sintético debe ir alineado");
    // hashes de superblock reales (como los calcularía makerom)
    {
        use sha2::{Digest, Sha256};
        ncch[0x1C0..0x1E0].copy_from_slice(&Sha256::digest(&exefs[..0x200]));
        ncch[0x1E0..0x200].copy_from_slice(&Sha256::digest(&romfs[..0x200]));
    }

    let p0_len = romfs_off + romfs_len;
    assert_eq!(p0_len % 0x200, 0);
    // --- CCI ---
    let cci = dir.join("base.3ds");
    {
        let mut f = std::fs::File::create(&cci).unwrap();
        f.write_all(&vec![0u8; 0x4000]).unwrap();
        f.seek(SeekFrom::Start(ncch_off)).unwrap();
        f.write_all(&ncch).unwrap();
        f.write_all(&exefs).unwrap();
        f.write_all(&romfs).unwrap();
        f.flush().unwrap();
    }
    // cabecera NCSD real (1 slot)
    {
        let mut hdr = [0u8; 0x200];
        cci::write_ncsd_header(
            &mut hdr,
            &[(ncch_off / 0x200, p0_len / 0x200)],
            0x00040000000EDF00,
            cci::card_media_size(0x4000 + p0_len).unwrap(),
        );
        let mut f = std::fs::OpenOptions::new().write(true).open(&cci).unwrap();
        f.write_all(&hdr).unwrap();
    }
    (cci, romfs_off, romfs_len)
}

/// Genera un .xdelta con el binario del sistema (como haría el traductor).
fn make_patch(src: &[u8], dst: &[u8], dir: &Path, name: &str) -> PathBuf {
    let s = dir.join(format!("{name}.src"));
    let d = dir.join(format!("{name}.dst"));
    let p = dir.join(format!("{name}.xdelta"));
    std::fs::write(&s, src).unwrap();
    std::fs::write(&d, dst).unwrap();
    let st = std::process::Command::new("xdelta3")
        .args(["-e", "-s"])
        .arg(&s)
        .arg(&d)
        .arg(&p)
        .status()
        .expect("xdelta3 en PATH para este test");
    assert!(st.success());
    p
}

#[test]
fn pack_sintetico_end_to_end() {
    let dir = std::env::temp_dir().join(format!("ie-msyn-{}.d", std::process::id()));
    eprintln!("DIR={}", dir.display());
    std::fs::create_dir_all(&dir).unwrap();
    let (base, _, _) = build_synth(&dir);

    // Pack: f1 AAAAA -> 7 bytes (crece; f2 debe desplazarse).
    let new_f1 = b"HHHHHHH";
    let patch = make_patch(b"AAAAA", new_f1, &dir, "f1");
    let pack = dir.join("pack");
    std::fs::create_dir_all(pack.join("parches")).unwrap();
    std::fs::copy(&patch, pack.join("parches/f1.xdelta")).unwrap();
    let mf = format!(
        r#"{{"version":"t","base":"s","ficheros":[{{"ruta":"f1","original_size":5,"original_sha256":"{o}","resultado_size":7,"resultado_sha256":"{r}","parche":"parches/f1.xdelta"}}]}}"#,
        o = sha(b"AAAAA"),
        r = sha(new_f1)
    );
    std::fs::write(pack.join("manifiesto.json"), mf).unwrap();
    // el manifiesto también se valida por la GUI
    assert!(manifest::describe(&pack).is_ok());

    let out = dir.join("out.3ds");
    let cancel = AtomicBool::new(false);
    pipeline::run_manifest(
        &pipeline::ManifestInputs {
            base: base.clone(),
            pack_dir: pack.clone(),
            out_cci: out.clone(),
            keep_work: false,
        },
        &cancel,
        &mut |p| {
            eprintln!("etapa {}/{}: {} {}/{}", p.stage_idx + 1, p.stage_count, p.stage, p.done, p.total);
        },
    )
    .expect("run_manifest sintetico");

    // Verifica: particiones, IVFC no aplica aquí (blob propio), contenido.
    let slots = cci::partitions(&out).unwrap();
    assert_eq!(slots.len(), 1);
    let (p0, _) = slots[0];
    let mut f = std::fs::File::open(&out).unwrap();
    // NCCH intacto
    f.seek(SeekFrom::Start(p0 + 0x100)).unwrap();
    let mut mg = [0u8; 4];
    f.read_exact(&mut mg).unwrap();
    assert_eq!(&mg, b"NCCH");
    // Lee el RomFS nuevo y comprueba f1 nuevo + f2 desplazado intacto.
    let mut nh = [0u8; 0x200];
    f.seek(SeekFrom::Start(p0)).unwrap();
    f.read_exact(&mut nh).unwrap();
    let media = 1u64 << (nh[0x18E] + 9);
    let rmo = u32::from_le_bytes(nh[0x1B0..0x1B4].try_into().unwrap()) as u64 * media;
    let rms = u32::from_le_bytes(nh[0x1B4..0x1B8].try_into().unwrap()) as u64 * media;
    let mut blob = vec![0u8; rms as usize];
    f.seek(SeekFrom::Start(p0 + rmo)).unwrap();
    f.read_exact(&mut blob).unwrap();
    // entrada f1: doff=old_end alineado, len 7
    let e1 = u64::from_le_bytes(blob[0x218..0x220].try_into().unwrap());
    let l1 = u64::from_le_bytes(blob[0x220..0x228].try_into().unwrap());
    assert_eq!(l1, 7);
    assert_eq!(&blob[0x300 + e1 as usize..0x300 + e1 as usize + 7], new_f1);
    // entrada f2: mismo doff 8, bytes intactos
    let e2 = u64::from_le_bytes(blob[0x258..0x260].try_into().unwrap());
    assert_eq!(e2, 8);
    assert_eq!(&blob[0x300 + 8..0x300 + 12], b"1234");
    std::fs::remove_dir_all(&dir).ok();
}
