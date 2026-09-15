<!-- SPDX-FileCopyrightText: 2026 Javiju -->
<!-- SPDX-License-Identifier: MIT -->

# Software y datos de terceros

Este proyecto es MIT (ver `LICENSE`), pero integra o referencia piezas de
terceros. Resumen honesto de cada una:

## xdelta3 (Apache 2.0) — empaquetado

- Qué: binarios oficiales v3.2.0 (Linux/Windows/macOS-arm64) en
  `gui/sidecars/`, usados como sidecar para el decode VCDIFF.
- Origen: https://github.com/jmacd/xdelta (© Joshua MacDonald y colaboradores).
- Licencia: Apache License 2.0, con copia exacta junto a cada binario
  (`gui/sidecars/*/LICENSE`, SPDX `Apache-2.0`).
- No se modifica ni se enlaza: se ejecuta como proceso externo.

## Claves AES de 3DS — constantes funcionales

- Qué: KeyX retail y constante del scrambler en
  `gui/crates/ie-core/src/ncch.rs`.
- Origen: mismos valores que distribuye 3dstool en su fuente abierta
  (dnasdw/3dstool, `src/ncch.cpp`). Son constantes funcionales de 16 bytes
  sin las cuales el descifrado es imposible; todo tool 3DS las incluye.
- Sin ellas la app no puede descifrar la base del propio usuario.

## Relleno NCSD de makerom — 256 bytes de salida de herramienta

- Qué: `gui/crates/ie-core/src/ncsd_rsa_filler.bin`, emitidos tal cual en la
  cabecera de cada CCI generado (SPDX `LicenseRef-MakeromOutput`).
- Origen: primeros 256 bytes de un CCI producido por `makerom -ciatocci`.
  Salida mecánica de una herramienta, no código ni contenido del juego;
  Azahar exige el campo presente (verificado empíricamente).

## Lo que NO se distribuye aquí (a diferencia de otras herramientas)

- DeltaPatcher (GPL-2.0): Luis lo empaqueta en su proyecto; nosotros usamos
  xdelta3 (Apache-2.0) y no lo necesitamos.
- ctrtool / 3dstool / makerom: no se empaquetan; su comportamiento
  competente (parseo, AES-CTR, layout NCSD) está reimplementado en
  `ie-core` a partir de su fuente abierta y de la documentación de 3dbrew.
- ROMs, CIAs ni ningún dato del juego: nunca en este repo (ver `.gitignore`
  y el paso `reuse lint` en CI).
