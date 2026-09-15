// SPDX-FileCopyrightText: 2026 Javiju
//
// SPDX-License-Identifier: MIT

//! Motor de reconstrucción/traducción de juegos de 3DS (probado a fondo con
//! Inazuma Eleven GO Galaxy en español).
//!
//! Nativo: parseo de CIA, descifrado NCCH (AES-CTR), construcción de CCI,
//! normalización a CCI descifrado y pipelines con progreso. El decode VCDIFF
//! se delega al binario oficial `xdelta3` (ver [`xdelta`]).
//!
//! Dos pipelines: [`pipeline::run`] (modo Galaxy, forzado `-n`) y
//! [`pipeline::run_strict`] (genérico: cadena de parches con checksums + hash
//! final, para parches sobre CCI canónico).

pub mod cci;
pub mod cia;
pub mod error;
pub mod manifest;
pub mod ncch;
pub mod normalize;
pub mod pipeline;
pub mod romfs;
pub mod xdelta;
