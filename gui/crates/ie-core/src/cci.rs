//! Construcción de CCI (.3ds, contenedor NCSD).
//!
//! Replica el layout que genera `makerom -ciatocci` (medido byte a byte sobre
//! su salida): cabecera NCSD de 0x200, partición 0 en el offset de media
//! 0x20 (= 0x4000) y partición 1 contigua.
//!
//! Dos detalles que SÍ importan aunque parezcan relleno (verificado
//! empíricamente: sin ellos Azahar dice "formato de aplicación inválido"):
//! - Los 256 bytes de firma NCSD no pueden ir a cero: se emite el mismo
//!   relleno que escribe makerom (`ncsd_rsa_filler.bin`, primeros 256 bytes
//!   de un CCI suyo; es salida de la herramienta, no contenido del juego).
//! - El segundo id de la tabla (0x198) es el title id con el tipo
//!   0x0004 -> 0x0005 en la palabra alta, tal cual lo emite makerom.

use crate::error::{Error, Result};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

pub const MEDIA_UNIT: u64 = 0x200;
const PART0_MEDIA_OFF: u64 = 0x20; // = 0x4000 bytes

fn u32le(v: u32) -> [u8; 4] {
    v.to_le_bytes()
}

/// Copia un fichero entero con progreso (devuelve bytes copiados).
fn copy_with_progress(
    src: &Path,
    dst: &mut File,
    cancel: &AtomicBool,
    on_chunk: &mut dyn FnMut(u64),
) -> Result<()> {
    let mut f = File::open(src)?;
    let mut buf = vec![0u8; 64 * 1024 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        dst.write_all(&buf[..n])?;
        on_chunk(n as u64);
    }
    Ok(())
}

/// Construye el CCI en `out` a partir de los contents ya listos.
/// `title_id` sale de la cabecera NCCH del content0.
pub fn build_cci(
    content0: &Path,
    content1: &Path,
    title_id: u64,
    out: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<()> {
    let c0_len = std::fs::metadata(content0)?.len();
    let c1_len = std::fs::metadata(content1)?.len();
    for (name, len) in [("content0", c0_len), ("content1", c1_len)] {
        if len % MEDIA_UNIT != 0 {
            return Err(Error::Format(format!("{name} no alineado a media unit: {len:#x}")));
        }
    }
    let p0_off = PART0_MEDIA_OFF;
    let p0_size = c0_len / MEDIA_UNIT;
    let p1_off = p0_off + p0_size;
    let p1_size = c1_len / MEDIA_UNIT;
    let grand = c0_len + c1_len;

    let mut f = OpenOptions::new().write(true).create(true).truncate(true).open(out)?;
    // Hueco para la cabecera NCSD (se rellena al final).
    f.write_all(&vec![0u8; 0x4000])?;
    let mut done = 0u64;
    {
        let mut report = |d: u64| {
            done += d;
            progress(done, grand);
        };
        copy_with_progress(content0, &mut f, cancel, &mut report)?;
        copy_with_progress(content1, &mut f, cancel, &mut report)?;
    }

    // Cabecera NCSD (0x200, resto del bloque 0x4000 ya a cero).
    // La firma no puede ir a cero (Azahar rechaza el CCI); se usa el relleno
    // de makerom, ver comentario del módulo.
    const RSA_FILLER: &[u8; 256] = include_bytes!("ncsd_rsa_filler.bin");
    let mut hdr = [0u8; 0x200];
    hdr[0x000..0x100].copy_from_slice(RSA_FILLER);
    hdr[0x100..0x104].copy_from_slice(b"NCSD");
    hdr[0x104..0x108].copy_from_slice(&u32le(0x800000)); // capacidad tarjeta 4GB (como makerom)
    hdr[0x108..0x110].copy_from_slice(&title_id.to_le_bytes());
    hdr[0x120..0x124].copy_from_slice(&u32le(p0_off as u32));
    hdr[0x124..0x128].copy_from_slice(&u32le(p0_size as u32));
    hdr[0x128..0x12C].copy_from_slice(&u32le(p1_off as u32));
    hdr[0x12C..0x130].copy_from_slice(&u32le(p1_size as u32));
    hdr[0x188..0x190].copy_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x01, 0x01, 0x00, 0x00]);
    hdr[0x190..0x198].copy_from_slice(&title_id.to_le_bytes());
    // Segundo id: palabra alta 0x0004 -> 0x0005 (tipo manual/CFA),
    // tal cual lo emite makerom.
    let second = (title_id & 0x0000_FFFF_FFFF_FFFF) | 0x0005_0000_0000_0000u64;
    hdr[0x198..0x1A0].copy_from_slice(&second.to_le_bytes());
    f.seek(SeekFrom::Start(0))?;
    f.write_all(&hdr)?;
    f.flush()?;
    progress(grand, grand);
    Ok(())
}

/// Tabla de particiones del CCI: [(offset_bytes, size_bytes)].
pub fn partitions(path: &Path) -> Result<Vec<(u64, u64)>> {
    let mut f = File::open(path)?;
    let mut hdr = [0u8; 0x200];
    f.read_exact(&mut hdr)?;
    if &hdr[0x100..0x104] != b"NCSD" {
        return Err(Error::Format("no es un CCI (sin magia NCSD)".into()));
    }
    let mut out = Vec::new();
    for i in 0..8 {
        let o = 0x120 + i * 8;
        let off = u32::from_le_bytes(hdr[o..o + 4].try_into().unwrap()) as u64 * MEDIA_UNIT;
        let size = u32::from_le_bytes(hdr[o + 4..o + 8].try_into().unwrap()) as u64 * MEDIA_UNIT;
        if size > 0 {
            out.push((off, size));
        }
    }
    Ok(out)
}

/// Autocomprobación estructural del CCI generado: magia, tabla de
/// particiones, magias NCCH y hashes de superblock ExeFS/RomFS.
pub fn verify_cci(path: &Path) -> Result<()> {
    let mut f = File::open(path)?;
    let mut hdr = [0u8; 0x200];
    f.read_exact(&mut hdr)?;
    if &hdr[0x100..0x104] != b"NCSD" {
        return Err(Error::Verify("el .3ds no tiene magia NCSD".into()));
    }
    let p0_off = u32::from_le_bytes(hdr[0x120..0x124].try_into().unwrap()) as u64 * MEDIA_UNIT;
    let p0_size = u32::from_le_bytes(hdr[0x124..0x128].try_into().unwrap()) as u64 * MEDIA_UNIT;
    for (off, size, name) in [
        (p0_off, p0_size, "content0"),
        (
            u32::from_le_bytes(hdr[0x128..0x12C].try_into().unwrap()) as u64 * MEDIA_UNIT,
            u32::from_le_bytes(hdr[0x12C..0x130].try_into().unwrap()) as u64 * MEDIA_UNIT,
            "content1",
        ),
    ] {
        let mut magic = [0u8; 0x200];
        f.seek(SeekFrom::Start(off))?;
        f.read_exact(&mut magic)?;
        if u32::from_le_bytes(magic[0x100..0x104].try_into().unwrap()) != 0x4843434E {
            return Err(Error::Verify(format!("{name} no es NCCH válido en el CCI")));
        }
        let _ = size;
    }
    verify_ncch_hashes(&mut f, p0_off)?;
    Ok(())
}

/// Recomputa los hashes de superblock del NCCH en `cxi_off` y los compara.
fn verify_ncch_hashes(f: &mut File, cxi_off: u64) -> Result<()> {
    let mut hdr = [0u8; 0x200];
    f.seek(SeekFrom::Start(cxi_off))?;
    f.read_exact(&mut hdr)?;
    let le = |o: usize| u32::from_le_bytes(hdr[o..o + 4].try_into().unwrap()) as u64;
    let media = 1u64 << (hdr[0x18E] + 9);
    let exo = le(0x1A0) * media;
    let exhs = le(0x1A8) * media;
    let rmo = le(0x1B0) * media;
    let rmhs = le(0x1B8) * media;
    let check = |f: &mut File, base: u64, len: u64, want: &[u8], what: &str| -> Result<()> {
        let mut h = Sha256::new();
        let mut left = len;
        let mut buf = vec![0u8; 8 * 1024 * 1024];
        f.seek(SeekFrom::Start(cxi_off + base))?;
        while left > 0 {
            let n = left.min(buf.len() as u64) as usize;
            f.read_exact(&mut buf[..n])?;
            h.update(&buf[..n]);
            left -= n as u64;
        }
        if h.finalize().as_slice() != want {
            return Err(Error::Verify(format!("hash de {what} no coincide")));
        }
        Ok(())
    };
    // La región de hash cubre desde el inicio de ExeFS/RomFS (superblock).
    check(f, exo, exhs, &hdr[0x1C0..0x1E0], "ExeFS")?;
    check(f, rmo, rmhs, &hdr[0x1E0..0x200], "RomFS")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn particiones_contiguas() {
        let dir = std::env::temp_dir();
        let c0 = dir.join("ie-cci-t0.bin");
        let c1 = dir.join("ie-cci-t1.bin");
        let out = dir.join("ie-cci-t-out.3ds");
        std::fs::write(&c0, vec![0xAAu8; 0x400]).unwrap();
        std::fs::write(&c1, vec![0xBBu8; 0x200]).unwrap();
        build_cci(&c0, &c1, 0x0004_0000_0010_BB00, &out, &AtomicBool::new(false), &mut |_, _| {}).unwrap();
        let got = std::fs::read(&out).unwrap();
        assert_eq!(got.len(), 0x4000 + 0x400 + 0x200);
        assert_eq!(&got[0x100..0x104], b"NCSD");
        assert_eq!(&got[0x4000..0x4002], &[0xAA, 0xAA]);
        assert_eq!(&got[0x4400..0x4402], &[0xBB, 0xBB]);
        assert_eq!(u64::from_le_bytes(got[0x108..0x110].try_into().unwrap()), 0x0004_0000_0010_BB00);
        for p in [&c0, &c1, &out] {
            std::fs::remove_file(p).ok();
        }
    }
}
