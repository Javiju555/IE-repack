# Nota 2026-09-14 (noche): modo `--xdelta` por decode forzado — DE HECHO SÍ FUNCIONA

> **Actualización (misma noche, tras la primera versión de esta nota, que
> concluía que no era viable): la vía SÍ funciona si antes se descifra la
> base in place. Lo que sigue es el análisis completo, con el error inicial
> incluido para que conste el camino.**

Objetivo del `PROMPT_AGENTE_IVFC.md`: que `rebuild.sh` funcione con **base JP +
`.xdelta` público**, sin necesitar un CIA ya traducido. Estado final:
**implementado y verificado** (`--xdelta-patch` en `scripts/rebuild.sh`,
probado end-to-end produciendo un `.3ds` válido con los `.fa` idénticos a la
referencia).

Objetivo del `PROMPT_AGENTE_IVFC.md`: que `rebuild.sh` funcione con **base JP +
`.xdelta` público**, sin necesitar un CIA ya traducido. Vía propuesta: `xdelta3
-d -n` (forzado) + reparar cabecera IVFC del RomFS.

## Lo que se reprodujo

- `xdelta3 -d -n -s <JP romsfun> parche_SN.xdelta forced_target.cia` produce
  3.005.760.512 bytes exactos (= tamaño del CIA ESP de terceros) y contiene
  texto español legible en claro (`strings | grep equipo`).
- Cabecera NCCH del forzado vs ESP real: solo **10 bytes** difieren
  (`0x18F` flags Secure→None, `0x1B4` byte alto del tamaño RomFS `B2`→`00`,
  `0x1B8`–`0x1BF` hash-region/unknown). Reparables sin referencia ESP
  (tamaño RomFS = `content0_size - romfs_offset`, resto copiable de la base JP).
- Cabecera IVFC del forzado: magia `IVFC` intacta, resto basura (17/96 bytes
  coinciden con ESP). Hasta aquí coincide con lo descrito en el prompt.

## Por qué reparar solo la cabecera no basta (hallazgo nuevo)

1. **El cifrado no es el único problema, pero sí parte de él.** La base JP de
   romsfun trae el NCCH en `Secure` y el ESP en `None`. Descifrando la base
   (3dstool extract + `--not-encrypt` + makerom, mismo tamaño) y re corriendo
   `check_adler.py`, se pasa de **0/358 a 41/358** ventanas coincidentes. El
   resto sigue fallando: los layouts a nivel CIA difieren entre
   `Supernovabase.cia` y el dump actual, no solo el cifrado.
2. **Los COPY del VCDIFF dependen del layout de la fuente.** El parche codifica
   coincidencias pequeñas (p. ej. la magia `ARC0` de los `.fa`) como COPY con
   offsets de `Supernovabase.cia`. Aplicado sobre otro dump, esos COPY caen en
   offsets equivocados y dejan **corrupciones pequeñas dispersas** dentro de
   archivos mayoritariamente ADD. Evidencia:
   - En el forzado, `ie6_b` arranca en el mismo offset que en el ESP
     (`0x155F7560`, magia `ARC0`, primeros 32 B idénticos) pero diverge a
     partir del byte 32.
   - `ie6_a` ni siquiera conserva la magia en el offset ESP (`B7E93EC4…` en
     vez de `ARC0`; solo 24 B intermedios coinciden por casualidad).
   - Los headers ESP completos de 64 B **no aparecen en ningún punto** del
     forzado (búsqueda en los 3 GB).
   - El Level-3 del RomFS forzado (en `RomFS+0x1000`) no coincide con el ESP.
   - El español hallado por `strings` vive en regiones ADD, pero el archivo
     `.fa` como unidad queda inválido (tablas con COPY corruptos).
3. **Conclusión (parcial, ver actualización arriba):** con la base cifrada, aunque
   se recalculara la cabecera IVFC desde cero (opción 1
   del prompt), los contenidos `.fa` extraídos seguirían inválidos. **Pero el
   problema era el cifrado, no el layout**: descifrando primero, los COPY
   caen en su sitio y todo cuadra (detalles en la actualización).

## Observación aparte (pendiente de verificar, no cambia la herramienta)

Parseando el Level-3 del RomFS (en `RomFS+0x1000`, válido tanto en la base JP
descifrada como en el ESP) con un parser propio (3491 ficheros en ambos,
mismos nombres), **10 ficheros difieren en tamaño**, no 2:

- `ev10_0660s.moflex` +15.109.099, `ev10_0660b.moflex` +14.980.128,
  `PV02.moflex` +13.351.526, `PV01.moflex` +11.108.111,
  `pv02.dspadpcm.bcstm` +8.617.216, `pv01.dspadpcm.bcstm` +6.440.384,
  `ie6_b.fa` −7.407.452, `op00a.moflex` −3.317.948, `op00b.moflex` −3.041.592,
  `ie6_a.fa` −244.664 (suma neta +55.594.808, coherente con el RomFS mayor).

Esto contradice el "solo 2 cambian" de `RESUMEN.md`/`README.md` para este par
concreto (JP romsfun vs ESP de terceros). Ya resuelto: esas diferencias extra
son **de versión de dump, no de la traducción** — el forzado desde la base
descifrada reproduce el ExeFS y los `.fa` del ESP de referencia al byte, y
conserva los media/manual/voces del dump del usuario (el parche no los toca).
El modo `--xdelta` del script hace exactamente eso; el `README.md` ya dice
"ExeFS + 2 archivos" en vez de "solo 2".

## Recomendación (actualizada: hecho)

- ~~**No implementar el modo `--xdelta-patch` por decode forzado.**~~
  **Implementado.** Receta que funciona, ya en `scripts/rebuild.sh`:
  1. `dd` del content0 + split con 3dstool (al extraer descifra).
  2. Splice in place sobre una copia de la base: exheader + ExeFS + RomFS
     descifrados en sus mismos offsets + byte de flags `0x18F: 00→04`.
     Mismo tamaño y layout exactos (verificado: `IVFC` en claro en `0x385940`).
  3. `xdelta3 -d -n -s spliced.cia parche.xdelta forced.cia`.
  4. Chequeo: TitleId(base) == TitleId(forced) + magia `IVFC` (aborta si el
     parche es de otro juego).
  5. Reempaquetar el CIA con makerom (**obligatorio**: el TMD del forzado trae
     los hashes del contenido original y `makerom -ciatocci` lo rechaza con
     `CIA content: 0x00000000 is corrupt` si no se regenera).
  6. `makerom -ciatocci` → `.3ds`.
- Verificación byte a byte contra el CIA ESP de referencia: cabecera NCCH,
  IVFC, Level-3, ExeFS y **ambos `.fa` (SHA256) idénticos**. El content0 solo
  difiere en 22 bloques de 1 MB (voces `whs*.bcstm`, versión de dump) y el
  content1/manual queda el del usuario (el parche no lo toca). Los modos
  `--translated-cia`/`--fa-dir` se conservan como alternativa sin parche.
- Si algún día se retoma, la vía no es reparar IVFC sino entender el VCDIFF a
  nivel de layout o el formato `ARC0` (ver `3ds-xfsatool`) para un rescate por
  contenido, validando cada `.fa` extraído (magia, tablas, desempaquetado).
