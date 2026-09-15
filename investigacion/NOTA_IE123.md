# Nota 2026-09-15: ¿vale la herramienta para Inazuma Eleven 1·2·3?

## Validación con dump real 2026-09-15 (Vimm/No-Intro, .cci descifrado, 2 GB exactos)

SHA-256 del dump: `35f7497051fd84740e019d40d3632d5943bd265d0a8a9bc77c01112b1b2bfd8d`
(frente al `79bf42d3...` de su base: **no coincide**, como se esperaba).

El modo estricto **rechaza correctamente** (`target window checksum
mismatch`, sin quimera). Diagnóstico fino del v1 (256 ventanas):

- **224/256 ventanas coinciden**, 12 sin ventana de copia, **20 fallan**.
- Las 20 se agrupan en 3 zonas: inicio de fichero (wrapper NCSD/NCCH),
  ~1248–1442 MB (cola de `archive.fa` + `ina_main3ogre.cro`, audios `.SAD`,
  `sound.pb`) y cola (~1928–1952 MB, hashes RomFS + padding `0xFF` de
  cartucho limpio).
- ~190 MB de ficheros de juego (CRO, audio, bancos) difieren de verdad:
  esto **no es varianza de linaje** (el linaje mueve cabeceras/padding,
  no el contenido de los ficheros) → el dump Vimm es **otra revisión** del
  juego que su base (o su base es una reconstrucción de su toolchain, cosa
  que él mismo documenta: "CIA convertida a NCSD... NO equivale al
  cartucho"). Forzar aquí mezclaría datos de dos revisiones: prohibido.
- Conclusión operativa: el mecanismo estricto funciona *como debe* (acepta
  lo idéntico, rechaza lo distinto con mensaje claro). Lo que falta no es
  código, es **rebasar su parche sobre el dump canónico** (este Vimm,
  No-Intro, hash de arriba): entonces todo poseedor del mismo dump aplica
  en estricto, y los de CIA mediante nuestra conversión + puerta de
  contenido (`tail -c +513`).
- Test que lo fija: `gui/crates/ie-core/tests/ie123_strict.rs` (ignorado,
  con sus hashes v1/v1.1). Hoy falla en el paso 1 por diseño; pasará cuando
  la base coincida.

Contexto: Luis Hidalgo Aguilar (@Luishidalgoa02, GitHub `luishidalgoa`)
preguntó en el hilo si el mismo problema/solución aplica a su traducción de
**Inazuma Eleven 1·2·3: Endou Mamoru Densetsu** (3DS, `CTR-P-AETJ`). Su repo
(`luishidalgoa/inazuma-eleven-123-spanish`, solo parche + herramientas, sin
ROMs) dice: base descifrada con SHA-256
`79bf42d3f22c6d9e7c7234919ee84007e346fbf421d3faf7ab2b688a0fc4cba2`;
"no sirven las CIA ni las ROMs encriptadas".

## Intel del parche (sin necesitar ninguna ROM, todo público)

En `patch/` hay versiones v1, v1.1-update, v10, v22–v27 (1–22 MB). Cabeceras
VCDIFF (`xdelta3 printhdrs`):

- Fuente: `Inazuma Eleven 1-2-3 - Endou Mamoru Densetsu.3ds` (¡un **CCI
  descifrado**, no un CIA!).
- Target total: 2147483648 bytes exactos (2 GB) en v1 y v27.
- Ventanas de 8 MB con ADDs minúsculos (v1: 7845 bytes en ventana 0, 0 en
  ventana 1; v10: 1756; v27: 424 KB y luego 5). Traducción quirúrgica sobre
  contenedor canónico.
- v1.1 es una **cadena**: v1 sobre la ROM y luego `v1.1-update` sobre el
  resultado, con hashes intermedios/finales publicados
  (`73007ea1...` intermedio, `aa5a9f6c...` final).

## Diagnóstico (teoría, pendiente de un dump real para confirmar)

Mismo síntoma, causa prima hermana — con una diferencia de diseño importante:

- Galaxy: el parche compara un CIA entero (wrapper único por dump) → hizo
  falta modo forzado `-n` + splice-decrypt.
- IE123: el parche compara un CCI descifrado canónico (el dump de cartucho
  descifrado no tiene varianza de empaquetado como los CIA de la escena).
  Si la base descifrada del usuario es la misma versión, el xdelta **normal**
  (con checksums) aplica limpio. Y si falla, significa otra versión de
  verdad — forzar produciría una quimera, así que aquí lo correcto es modo
  **estricto** + hash final como guardián (Luis ya publica los hashes).

Predicción falsable: un CCI descifrado con SHA-256 `79bf42d3...` acepta sus
parches con `xdelta3 -d` a secas. No pudimos probarla: no hay dump de este
juego en la máquina y no descargamos ROMs.

## Hallazgo propio al implementar: el wrapper NCSD rompe el hash completo

Leyendo su repo, su build parchea `archive.fa` **in-place** sobre una copia
del CCI: el wrapper (firma RSA real del cartucho en `[0..0x100]`, flags de
tarjeta) queda intacto en su referencia. Nuestra conversión desde CIA genera
un wrapper válido pero distinto (relleno makerom + `media_size` por tarjeta,
ahora dinámico: 128 MB–8 GB según imagen). Consecuencia:

- Con base **cartucho** descifrada, nuestro hash completo SÍ puede coincidir
  con el suyo publicado (`aa5a9f...`): el wrapper se preserva tal cual.
- Con base **CIA convertida**, el hash completo no puede coincidir nunca
  (firma distinta), aunque el juego sea byte-idéntico. Por eso la puerta
  acepta también un **SHA-256 de contenido** (desde `0x200`, sin cabecera):
  un proyecto lo publica con `tail -c +513 resultado.3ds | sha256sum`.
  Implementado y probado con fixtures sintéticos (mismo contenido con
  wrapper tocado: pasa contenido, falla completo, como debe).

Esto cuadra además con su propia nota en `docs/PROGRESO.md`: "CIA convertida
localmente a NCSD... La nueva base NO equivale al cartucho usado para los
xdelta antiguos". Exacto: equivale en contenido, no en wrapper.

## Auditoría: qué era específico de Galaxy

Casi nada en el motor: `ie-core` (cia/ncch/cci/xdelta/pipeline) ya calculaba
todo dinámicamente y solo comparaba TitleId base-vs-resultado, nunca fijo.
Específico de Galaxy era solo presentación: tags `BGSJ`/`BGBJ` en la GUI,
título de ventana, nombre de salida por defecto y los fixtures del test
dorado (hashes de `ie6_a.fa`/`ie6_b.fa`).

## Decisión: mismo repo (opción 1 del prompt), ya implementado

- `ie-core`: `normalize` (entrada CIA **o** CCI → CCI descifrado canónico),
  `decode_strict` (sin `-n`, con stderr capturado para diagnosticar),
  `sha256_file`, `run_strict` (cadena de parches + hash final + rename
  atómico, sin temporales de contents).
- GUI: selector de modo. Galaxy igual que siempre; modo genérico
  "Parche .3ds (estricto, experimental)": base `.cia`/`.3ds`, lista ordenada
  de parches multi-selección con borrado por fila, SHA esperado opcional.
- Tests: `strict_synthetic` (cadena v1+v1.1 sintética + rechazo de base
  equivocada) corre en CI con xdelta3 de apt; el round-trip con bytes reales
  queda para cuando haya dump (ver abajo). Layout de ambos modos cubierto
  por el test kittest.

## Qué falta para dar IE123 por soportado (bloqueado, nada técnico)

1. Que alguien con el juego mida su base descifrada contra `79bf42d3...`
   (un `sha256sum`, 1 minuto) — si coincide, el modo genérico debería
   funcionar a la primera.
2. Ideal: una copia ya parcheada de referencia (Luis) para verificación
   byte a byte como hicimos con Galaxy.
3. Alternativa en casa: si aparece dump + parche de **Big Bang**, valida el
   mismo camino genérico sin depender de terceros.

## Segunda pasada 2026-09-15 (con herramientas DEL propio Luis, solo lectura)

- `code.bin` (ExeFS, BLZ) de nuestro dump descomprime con su `blz.py` y los
  tres hooks de `patch_code.py` caen en prólogos ARM de libro: `strcpy`
  @0x14AC5C (`ldrb r2,[r1]; mov r3,r0; cmp r2,#0; beq`), `getc` @0x1B3788
  (`ldr r1,[r0,#0x10]; add; str; ldrb`), `strcmp` @0x184AAC (`and r3,r0,#3;
  and ip,r1,#3; cmp; mov r2,#0). **Código idéntico a su base.**
- El B123 principal (ROM 0x162140, `data_off` 0x93b14) abre con su
  `fa_unpack.py`: **15.547 ficheros**, árbol `inazuma1/2/3(_ogre/_blizzard)`,
  `menu`, `font`, con `eve.pkb/pkh` presentes (IE1: 1293 eventos, incluye
  92010100/92010200 de su probe).
- Evento 10010001 de NUESTRO `eve.pkb` (descomprimido con su `lz10.py`,
  parseado con su `ssd_records.py`): **19/19 diálogos japoneses idénticos**
  a su `translation/game1/dialogo.csv`. **Scripts idénticos a su base.**
- Único B123 top-level en toda la ROM; los otros 16 `B123` son sub-archivos
  anidados (p.ej. dentro de `inazuma2/.../*.arc`). Sin CROs sueltos ni rutas
  `cro/` en el árbol (su `work/romfs/cro/ina_main1.cro` no existe aquí;
  los `CRO0` visibles son coincidencias en blobs comprimidos como
  `psh.pkb`, o nombres de entrada anidadas).
- Fecha empotrada `(2012/10/26` en `_PARAM_` (code + archive) y rutas de
  build `ina_main1\src\...` como huellas extra.

## Hipótesis revisada: NO es otra revisión, es base reconstruida

- Código ✓ y scripts ✓ idénticos a dump limpio No-Intro ⇒ la divergencia
  (20 ventanas: wrapper, ~190 MB CRO/audio/cola de datos, padding) está
  confinada a **medios y envoltorio**: justo lo que tocan sus herramientas
  (`fa_repack`, `mods_to_moflex`, `build_ie1_movies`, `ie1_media`) + su
  propia nota de "base reconstruida que no equivale al cartucho".
- Sigue valiendo no forzar (mezclaría su rebuild con el dump limpio), pero
  el mensaje para Luis cambia: no "tienes otra revisión" sino "tu base
  diverge de un No-Intro limpio solo en medios/wrapper: rebasea el parche
  sobre el dump canónico y todo cuadra". Pedirle: SHA de contenido de su
  v1.1 + de dónde salió exactamente su base (¿pasó por su toolchain?).

## Respuesta de Luis (2026-09-15) + giro a file-level

- Su parche = xdelta sobre ROM `.3ds` JP descifrada (CTR-P-AETJ). ExeFS
  intacto; todo en RomFS: `archive.fa` **reconstruido entero** (en un CIA no
  hay trozos sueltos que coincidan), `cro/ina_main1.cro` (mismo tamaño,
  pocos bytes), 70 `.SAD` (voces EU, mismas rutas).
- Confirma hipótesis revisada y explica los 20 fallos: su archive.fa
  objetivo es un rebuild completo (ventanas ADD + COPYs parciales que no
  casan con dump limpio). El xdelta-sobre-CIA es callejón sin salida.
- **Propone colaboración**: si avanzamos y es compatible con descifrado
  `.3ds`, sus próximas compilaciones saldrían en el mismo paquete con
  nuestra herramienta portable.
- Verificado además: `archive.fa` y `cro/ina_main1.cro` existen como
  ficheros SUELTOS en el RomFS del dump limpio (entradas UTF-16 únicas en
  0x14bdbc / 0x14be6c; FileData base = ROM 0x162140). `archive.fa` =
  [0x162140, 1309227584) 1.247 GB; `ina_main1.cro` @0x4E093940, 2105344 B,
  con `ldrb r0,[r4]` en 0x36CC0 ✓ y 716 ceros en 0x50E14 ✓ (magia inicial
  `99 4E 7F 8E`, no `CRO0` — preguntarle si su `.orig` empieza por CRO0).
- **Estrategia nueva (modelo Galaxy, sin xdelta)**: normalizar CIA/CCI →
  CCI descifrado y REEMPLAZAR ficheros RomFS (archive.fa + CRO + 70 SAD)
  + reconstruir RomFS (rehash IVFC, cambia el tamaño) + reempaquetar.
  Pedirle: set parcheado final + manifiesto (rutas + SHA-256) + tamaño de
  su archive.fa v1.1 + cómo quiere créditos/licencia del paquete conjunto.

## Prototipo file-level VERIFICADO 2026-09-15 (scratch, no en repo)

- Con 3dstool+ctrtool+Azahar del sistema: round-trip CXI extract→create
  **byte-idéntico**; FileData base = ROM 0x162140 (archive.fa dataoff 0);
  FileMeta localizado por nombre UTF-16 + tamaño (1660 rutas, 1690
  entradas con dupes); layout secuencial sin gaps relevantes.
- **Demo A** (swap 2 `.SAD` mismo tamaño): rebuild + splice → ctrtool
  GOOD + **arranca en Azahar hasta el título** (captura).
- **Demo B** (archive.fa +1 MB, 1660 entradas reubicadas, tabla NCSD
  actualizada): oráculo 3dstool confirma tamaños/offsets/contenidos,
  ctrtool GOOD, 60 s de ejecución sin errores.
- Pendiente producto: `romfs.rs` nativo (parser + rebuild + splice,
  validado contra este prototipo), modo GUI "reemplazo de ficheros" con
  manifiesto (ruta + SHA-256), y campo card-size si el archive.fa final
  supera los 2 GB. IVFC L1/L2 del dump original no casan con el layout
  estándar (superblock sí verifica); Azahar no lo exige — anotado como
  riesgo HW, a revalidar con el set real.

## v55/v57 INTEGRADO Y VERIFICADO 2026-09-15 (paquete_v55.rar, 79 MB)

- Formato: xdelta por fichero + `manifiesto.json` (ruta, sizes y SHA-256
  original/resultado). 200 ficheros: archive.fa, cro/ina_main1.cro,
  inazuma1/data_iz/sound/*.SAD sueltos (no van dentro de archive.fa).
- Gate: **200/200 SHA originales coinciden** con dump limpio (su base =
  nuestro dump a nivel fichero). Decode estricto 200/200 + gate resultado
  200/200. Traducción verificada a nivel bytes (eventos 92010100,
  10010002 en español decodificado).
- Rebuild: 1660 rutas / 1690 entradas localizadas (voto parent↔dir para
  dupes; mapping 200/200 byte-exacto); delta total +197.742.716;
  imagen final 2.330.284.032 (campo tarjeta → 4 GB, 0x800000).
- Verificación: re-extracción 200/200 SHAs, ctrtool GOOD, **arranca en
  Azahar EN ESPAÑOL** (título "La leyenda de Mark Evans", 55 FPS).
- Detalles para Luis: rar dice v55, manifiesto v57-interna (unificar);
  evento 10010001 aún en JP (normal, dev); CRO tiene cabecera 0x80 + CRO0
  en 0x80 (misterio resuelto); su formato por-fichero+manifiesto es ideal,
  sin cambios necesarios.

## Producto nativo VERIFICADO 2026-09-16 (modo Pack en la GUI)

- `ie-core`: `manifest.rs` (parse+validación), `romfs.rs` (localizar por
  contenido, descubrir base, rebuild por apéndice), `run_manifest`
  (normaliza, 200 puertas dobles, reensambla, re-verifica desde la imagen).
- Episodio loader: el nativo con cabecera regenerada dio una vez "formato
  inválido" en Azahar y luego arrancó siempre (4/4 boots incl. el mismo
  fichero). Causa no aislada (RSA y 0x1C8 descartados por bisección);
  blindaje aplicado: se preserva la cabecera NCSD original y solo se
  parchean slots+tarjeta. Si recurre, mirar `azahar_log.txt` (Loader).
- Tests: units (manifiesto, romfs sintético), `manifest_synth` (pack de 1
  fichero end-to-end con shifts, CI con xdelta3), `manifest_v55` (ignorado,
  200/200 reales). GUI: modo Pack + guía + pie + log a fichero siempre
  activo + test de layout a 780 px en los 3 modos.
- Pendiente: probarlo el usuario, push, que Luis lo pruebe a fondo; fase 2:
  salida cifrada/CIA para flashcart/CFW.
- CIA N-contenidos 2026-09-16: normaliza descifrando cada CXI con ExeFS
  (los CFA sin ExeFS, p. ej. manuales, viajan como en Galaxy: cifrados,
  Azahar arranca igual); slots 0..N-1 por orden (el del manual puede
  diferir del cartucho, cosmético). Validado con CIA cifrado real de
  Galaxy + minipack de 1 fichero (`manifest_galaxy_cia`, ignorado).
  IE123-CIA de 3 contenidos: pendiente de muestra real para validar slots.
- CRO hashes 0x80: la herramienta aplica los bytes tal cual (no recalcula,
  igual que su pipeline). En Azahar funciona; en HW con Luma los CRO
  parcheados funcionan en la práctica (la escena los usa a diario).
- Ojo versiones: su mensaje dice archive v55 ≈1.454.492.944 pero el
  manifiesto v57 declara resultado 1.505.186.464. El manifiesto manda;
  pedirle que unifique nombres (rar v55 vs manifiesto v57).

## Borrador de respuesta para el hilo (publica Javiju, no el agente)

> Mirado a fondo, con dump real en mano (Vimm/No-Intro descifrado,
> `35f74970...`, que NO coincide con tu base): tu caso es el bueno para
> explicar por qué el modo estricto importa. Medido ventana a ventana, tu v1
> coincide en 224/256 con ese dump y falla en 20 agrupadas en wrapper, cola
> de `archive.fa`+CRO/audio y padding final. Eso huele a otra revisión del
> juego (o a tu base reconstruida, que ya documentas que no equivale al
> cartucho) — y forzar ahí mezclaría dos revisiones, así que nuestra
> herramienta se niega en vez de corromper.
>
> Actualización con dump en mano y tus propias herramientas (solo lectura):
> tu código y tus scripts son idénticos a un No-Intro limpio — lo verifiqué:
> tu `blz.py` abre mi `code.bin` y tus tres hooks caen en su sitio, y 19/19
> diálogos del evento 10010001 coinciden con tu `dialogo.csv`. La divergencia
> (20 ventanas de 256) está solo en medios/CRO/padding: pinta a que tu base
> pasó por tu toolchain (tu `fa_repack`/medios), no a otra revisión del juego.
> Si rebaseas el parche sobre el dump canónico, nuestro modo genérico lo
> aplica en estricto tal cual, con cadena v1→v1.1 y hash final; y si publicas
> además el SHA de contenido (`tail -c +513 resultado.3ds | sha256sum`),
> hasta los que vengan de CIA convertido pueden verificar al byte.
> ¡Gracias por el currazo de la traducción!
>
> PD: enhorabuena por la higiene del repo (LEGAL.md, sin ROMs, TERCEROS con
> licencias) — es exactamente como debería hacerse. Dos apuntes menores
> desde fuera: la sopa de versiones (v1/v1.1 vs v22-v27 vs builds v33/v35)
> confunde sobre qué cadena aplica a qué (tu `instrucciones-v1.1.txt` es el
> modelo a seguir: pasos + hashes intermedios); y el `Comprobar_ROM.bat`
> deja fuera a Linux/macOS, que es justo el hueco que cubre nuestra GUI.
