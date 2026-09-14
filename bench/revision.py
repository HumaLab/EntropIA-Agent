"""Página local de revisión de una tanda de candidatas del EntropIA-Bench.

Uso, desde la raíz del repo:  python <ruta>/revision.py <N>

Lee bench/candidatas/tanda-<N>.json y el corpus (entropia.sqlite, solo
lectura) y escribe bench/candidatas/tanda-<N>-revision.html, con el texto
completo de cada fragmento de evidencia para que el historiador lea y apruebe.
El HTML está ignorado por git: se genera, no se versiona.
"""

import html
import json
import io
import os
import sqlite3
import sys

CSS = """
:root {
  --fondo: #fbfaf8; --papel: #fff; --tinta: #1c1a17; --suave: #6b6560;
  --linea: #e4ded6; --acento: #8a5a2b; --ok: #2f6f4f; --alerta: #9a3b2f;
  --cita: #f6f2ec;
}
@media (prefers-color-scheme: dark) {
  :root {
    --fondo: #16151300; --fondo: #141312; --papel: #1c1b19; --tinta: #e8e3dc;
    --suave: #a29a91; --linea: #2e2b28; --acento: #d99a5b; --ok: #7fc0a0;
    --alerta: #e08a7a; --cita: #211f1c;
  }
}
* { box-sizing: border-box; }
body { margin: 0; background: var(--fondo); color: var(--tinta);
  font: 16px/1.6 Georgia, "Iowan Old Style", serif; }
.hoja { max-width: 900px; margin: 0 auto; padding: 32px 20px 80px; }
h1 { font-size: 1.7rem; margin: 0 0 4px; letter-spacing: -.01em; }
.sub { color: var(--suave); font-size: .95rem; margin: 0 0 28px; }
.sub code { font-family: ui-monospace, Menlo, Consolas, monospace; font-size: .85em; }
.ficha { background: var(--papel); border: 1px solid var(--linea);
  border-radius: 10px; padding: 22px 24px; margin: 0 0 22px; }
.cab { display: flex; flex-wrap: wrap; gap: 10px; align-items: baseline;
  border-bottom: 1px solid var(--linea); padding-bottom: 12px; margin-bottom: 16px; }
.id { font: 600 1.05rem ui-monospace, Menlo, Consolas, monospace; color: var(--acento); }
.etq { font-size: .78rem; text-transform: uppercase; letter-spacing: .08em;
  color: var(--suave); border: 1px solid var(--linea); border-radius: 999px;
  padding: 2px 9px; }
.pasa { color: var(--ok); border-color: var(--ok); }
.falla { color: var(--alerta); border-color: var(--alerta); }
.preg { font-size: 1.12rem; margin: 0 0 16px; }
.rot { font-size: .75rem; text-transform: uppercase; letter-spacing: .09em;
  color: var(--suave); margin: 18px 0 6px; }
.resp { margin: 0; }
.just { margin: 0; color: var(--suave); font-size: .93rem; }
.grupo { border-left: 3px solid var(--linea); padding-left: 14px; margin: 12px 0; }
.frag { background: var(--cita); border-radius: 8px; padding: 14px 16px; margin: 8px 0; }
.meta { font: .75rem/1.5 ui-monospace, Menlo, Consolas, monospace;
  color: var(--suave); margin-bottom: 8px; word-break: break-all; }
.txt { white-space: pre-wrap; font-size: .93rem; margin: 0;
  max-height: 22em; overflow-y: auto; }
.alt { font-size: .8rem; color: var(--suave); font-style: italic; margin: 0 0 6px; }
.desc li { margin-bottom: 12px; }
.desc .motivo { color: var(--suave); font-size: .92rem; }
hr { border: 0; border-top: 1px solid var(--linea); margin: 40px 0 28px; }
@media (max-width: 520px) { .hoja { padding: 20px 16px 60px; } }
"""


def texto_chunk(cur, chunk_id):
    fila = cur.execute(
        "select rc.text_content, i.title, co.name "
        "from rag_chunks rc join items i on i.id = rc.item_id "
        "left join collections co on co.id = i.collection_id where rc.id = ?",
        (chunk_id,),
    ).fetchone()
    if not fila:
        return None, "(no está en el corpus)", "?"
    return fila[0], fila[1], fila[2] or "sin colección"


def main():
    if len(sys.argv) < 2:
        sys.exit("uso: python revision.py <N>")
    n = sys.argv[1]
    origen = os.path.join("bench", "candidatas", f"tanda-{n}.json")
    destino = os.path.join("bench", "candidatas", f"tanda-{n}-revision.html")
    tanda = json.load(io.open(origen, encoding="utf-8"))
    con = sqlite3.connect("file:entropia.sqlite?mode=ro", uri=True)
    cur = con.cursor()

    p = []
    a = p.append
    a("<!doctype html><html lang='es'><head><meta charset='utf-8'>")
    a("<meta name='viewport' content='width=device-width,initial-scale=1'>")
    a(f"<title>Tanda {n} — revisión</title><style>{CSS}</style></head><body><div class='hoja'>")
    a(f"<h1>Tanda {n} — revisión del historiador</h1>")
    a(
        f"<p class='sub'>{len(tanda['candidatas'])} candidatas para "
        f"<code>{html.escape(tanda['banco_destino'])}</code>. "
        f"Estado: {html.escape(str(tanda.get('estado', '—')))}.<br>"
        f"Protocolo: {html.escape(str(tanda.get('protocolo', '—')))}</p>"
    )

    for c in tanda["candidatas"]:
        a("<div class='ficha'>")
        a("<div class='cab'>")
        a(f"<span class='id'>{html.escape(c['id'])}</span>")
        a(f"<span class='etq'>{html.escape(c['tipo'])}</span>")
        a(f"<span class='etq'>nivel {c.get('nivel', '?')}</span>")
        a("</div>")
        a(f"<p class='preg'>{html.escape(c['pregunta'])}</p>")
        a("<div class='rot'>Respuesta de referencia</div>")
        a(f"<p class='resp'>{html.escape(c.get('respuesta_referencia', ''))}</p>")
        if c.get("grupos_justificacion"):
            a("<div class='rot'>Por qué esos grupos</div>")
            a(f"<p class='just'>{html.escape(c['grupos_justificacion'])}</p>")

        grupos = c.get("grupos_esperados") or [[x] for x in c["chunk_ids_esperados"]]
        a(f"<div class='rot'>Evidencia — {len(grupos)} grupo(s)</div>")
        for i, grupo in enumerate(grupos, 1):
            a("<div class='grupo'>")
            if len(grupo) > 1:
                a(
                    f"<p class='alt'>Grupo {i}: {len(grupo)} fragmentos "
                    "alternativos, alcanza con recuperar uno.</p>"
                )
            else:
                a(f"<p class='alt'>Grupo {i}</p>")
            for chunk_id in grupo:
                txt, titulo, coleccion = texto_chunk(cur, chunk_id)
                a("<div class='frag'>")
                a(
                    f"<div class='meta'>{html.escape(coleccion)} › "
                    f"{html.escape(titulo)}<br>{html.escape(chunk_id)}</div>"
                )
                a(f"<p class='txt'>{html.escape(txt or '')}</p>")
                a("</div>")
            a("</div>")
        a("</div>")

    descartadas = tanda.get("descartadas") or []
    if descartadas:
        a("<hr><h1>Descartadas y reservas</h1>")
        a(f"<p class='sub'>{len(descartadas)} anotadas.</p><ol class='desc'>")
        for d in descartadas:
            a(
                f"<li>{html.escape(d.get('pregunta', ''))}"
                f"<br><span class='motivo'>{html.escape(d.get('motivo', ''))}</span></li>"
            )
        a("</ol>")

    a("</div></body></html>")
    io.open(destino, "w", encoding="utf-8", newline="\n").write("\n".join(p))
    print(destino)


if __name__ == "__main__":
    main()
