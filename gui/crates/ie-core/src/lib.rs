//! Lógica de reconstrucción de Inazuma Eleven GO Galaxy en español.
//!
//! Subconjunto nativo de lo que `scripts/rebuild.sh --xdelta-patch` hace con
//! `ctrtool`/`3dstool`/`makerom`: parseo de CIA, descifrado NCCH (AES-CTR) y
//! construcción de CCI. El decode VCDIFF se delega al binario oficial
//! `xdelta3` (ver [`xdelta`]).

pub mod cci;
pub mod cia;
pub mod error;
pub mod ncch;
pub mod pipeline;
pub mod xdelta;
