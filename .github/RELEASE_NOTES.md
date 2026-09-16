# IE Repack v0.2.0 — parcheador ES (Galaxy + packs por manifiesto)

Tu base japonesa (`.cia`/`.3ds`) + la traducción → `.3ds` en español listo
para Azahar. Tres modos a elegir en la pantalla de inicio: Galaxy
Supernova (parche único), Pack de traducción (carpeta o `.zip` con
`manifiesto.json`, verificado fichero a fichero) y Xdelta estricto
experimental. Portable, sin instalador: descomprime y ejecuta. Si algo
falla, adjunta `ie-repack.log` en un issue:
https://github.com/Javiju555/ie-repack/issues

Probado con Galaxy Supernova ES e IE 1-2-3 ES (arranque en español
verificado). Este proyecto no incluye ninguna ROM, CIA ni dato del juego.

---

# IE Repack v0.2.0-beta.1 — modo Pack (IE 1-2-3 ES, en pruebas)

**Versión de pruebas, no final.** Nuevo modo "Pack de traducción
(manifiesto)": base japonesa (`.cia`/`.3ds`) + carpeta del pack
(`manifiesto.json` + parches por fichero) → `.3ds` traducido. Verificado
con el pack interno v57 de IE 1-2-3 ES (200/200 ficheros, arranque en
español en Azahar). El modo Galaxy sigue igual.

Si algo falla, adjunta `ie-repack.log` (junto al programa) en un
issue: https://github.com/Javiju555/ie-repack/issues

---

# IE Galaxy Repack v0.1.0 — Galaxy Supernova en español, sin dramas

Reconstruye una versión traducida y jugable de **Inazuma Eleven GO Galaxy:
Supernova** a partir de **tu propio CIA japonés + el parche `.xdelta`
público** de @IEgogalaxyesp (v1.0.5). Sin terminal: abres la app, eliges los
dos archivos, pulsas el botón y sale un `.3ds` listo para **Archivo →
Cargar archivo** en Azahar.

## Qué hay en cada zip

- `ie-galaxy-repack` (o `.exe`): la aplicación gráfica.
- `sidecars/<tu-so>/xdelta3*`: el decodificador oficial usado por dentro.
- `LICENSE`: licencia MIT del proyecto (xdelta3 es Apache 2.0, con su
  licencia incluida junto a su binario).

## Uso mínimo

1. Consigue tu CIA japonés del juego y el `.zip` del parche del blog.
2. Ponlos junto al programa (los precarga solos) o elígelos con los botones.
3. Pulsa **Crear .3ds en español**, espera al `OK` y carga el `.3ds` en Azahar.
4. Hacen falta ~10 GB libres donde guardes la salida (mueve ~3 GB varias
   veces) y el proceso tarda de segundos a pocos minutos según el PC.

## Por qué existe

El parche oficial falla con checksum contra cualquier dump que no sea el
archivo concreto del traductor. Esta herramienta descifra tu base in place y
aplica el parche en modo forzado: verificado byte a byte contra una copia
traducida de referencia (cabecera NCCH, IVFC, ExeFS y ambos `.fa` idénticos).
Detalle completo en el `README.md` del repo.

Todo el crédito de la traducción es del equipo original liderado por
Ale_z_plumber. Este proyecto no incluye ninguna ROM, CIA ni dato del juego.
