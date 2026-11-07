# IE Repack v0.5.0-beta.1 — en pruebas, se busca gente que la pruebe

**Versión de pruebas, no final.** La estable sigue siendo la
[0.4.1](https://github.com/Javiju555/ie-repack/releases/tag/v0.4.1). La
herramienta se ha desarrollado y probado sobre todo en Linux y **no se ha
podido probar en Windows ni instalar el `.cia` en una 3DS real**. Si la
pruebas, cuéntalo en un issue, salga bien o mal:
https://github.com/Javiju555/ie-repack/issues (tu sistema, el modo que usaste
y, si falla, el `ie-repack.log` que se crea junto al programa).

## Qué cambia

- **Windows: sin ventana de CMD.** Ya no se abre la consola negra que, si se
  cerraba, cerraba también el programa. *Sin probar en Windows: es lo
  primero que hay que confirmar.*
- **Salida `.cia`** además del `.3ds`, si tienes un CIA del mismo juego como
  donante. Instala y arranca en Azahar. *En 3DS real, sin probar.*
- **Errores más claros cuando la base no vale:** dice si es otro juego u otra
  versión, si ya está traducida o si el volcado está incompleto, en vez de
  «ningún origen cuadra con el manifiesto».
- **Al terminar,** te dice dónde está el archivo y cómo abrirlo (Azahar o
  Lime3DS para `.3ds`; FBI en consola para `.cia`).
- Iconos y banner del juego también se parchean si el pack los trae, y el
  nombre de salida sale del propio juego cuando la base es un `.cia`.

## Qué hay en cada zip

- `ie-repack` (o `ie-repack.exe`): la aplicación. Portable, sin instalador.
- `sidecars/<tu-so>/`: el `xdelta3` oficial que usa por dentro. Descomprime
  el zip entero: el programa necesita esa carpeta al lado.

Este proyecto no incluye ninguna ROM, CIA ni dato del juego.
