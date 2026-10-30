/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 */
export function reportHtml(report, overlays) {
  const json = JSON.stringify({ report, overlays }).replaceAll("<", "\\u003c");
  return `<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width"><title>Engine G-code comparison</title>
<style>body{font:15px system-ui;max-width:1050px;margin:32px auto;padding:0 20px;background:#10151b;color:#e9edf3}p{line-height:1.5}table{width:100%;border-collapse:collapse}td,th{text-align:left;padding:10px;border-bottom:1px solid #425064}select,input{margin:12px;padding:8px}svg{width:min(100%,560px);height:480px;background:#fff}pre{white-space:pre-wrap;max-height:450px;overflow:auto}.s{color:#48b7fa}.r{color:#f18a3b}</style>
<h1>Engine G-code comparison</h1><p>Independent commanded XYZ/E observations for analytic 10 mm cubes. PrusaSlicer is a comparison, not an oracle. Correctness checks and unresolved differences are separate. Nominal footprint checks do not establish physical printability.</p>
<table><thead><tr><th>Case</th><th>Audit Spiling / reference</th><th>Layers</th><th>Model volume mm³</th><th>Differences</th></tr></thead><tbody id="rows"></tbody></table>
<p><select id="case"></select><label>Physical band <input id="band" type="range" min="0" max="49" value="0"></label><span id="height"></span></p>
<p><span class="s">Spiling</span> / <span class="r">reference</span> commanded centerlines. Every intersecting deposition interval in the selected 0.2 mm physical band is shown; a refined band may contain two paths.</p>
<svg viewBox="-.5 -.5 11 11" role="img" aria-label="Commanded deposition overlay"><g transform="translate(0 10) scale(1 -1)"><path d="M0 0H10V10H0Z" fill="none" stroke="#aaa" stroke-width=".025"/><path id="spiling" fill="none" stroke="#008ad4" stroke-width=".035" opacity=".8"/><path id="reference" fill="none" stroke="#dd6410" stroke-width=".025" opacity=".8"/></g></svg>
<pre id="detail"></pre><script>const data=${json};
const el=id=>document.getElementById(id); for(const c of data.report.cases){const tr=document.createElement('tr');const a=c.spiling,b=c.reference;const vals=[c.id,(a?String(a.correctnessPassed):'REFUSED/ERROR')+' / '+(b?String(b.correctnessPassed):'ERROR'),a&&b?a.layerCount+' / '+b.layerCount:'—',a&&b?a.modelVolumeMm3.toFixed(3)+' / '+b.modelVolumeMm3.toFixed(3):'—',c.error??c.comparison.differences.filter(d=>d.status==='unresolved').length+' unresolved']; for(const v of vals){const td=document.createElement('td');td.textContent=v;tr.append(td)}el('rows').append(tr);if(data.overlays[c.id]){const o=document.createElement('option');o.value=c.id;o.textContent=c.id;el('case').append(o)}}
function update(){const id=el('case').value,n=Number(el('band').value);el('height').textContent=(n*.2).toFixed(1)+'–'+((n+1)*.2).toFixed(1)+' mm';for(const name of ['spiling','reference'])el(name).setAttribute('d',data.overlays[id]?.[name]?.[n]??'');el('detail').textContent=JSON.stringify((()=>{const c=data.report.cases.find(c=>c.id===id);return {error:c?.error,spilingViolations:c?.spiling?.violations,referenceViolations:c?.reference?.violations,comparison:c?.comparison}})(),null,2)}el('case').onchange=update;el('band').oninput=update;update();</script></html>`;
}
export function overlayBands(decoded) {
  const model = decoded.strokes.filter(
    (s) => s.from.every((v) => v >= 0 && v <= 10) && s.to.every((v) => v >= 0 && v <= 10),
  );
  const zs = [...new Set(model.map((s) => s.to[2]))].sort((a, b) => a - b);
  return Array.from({ length: 50 }, (_, i) => {
    const included = zs.filter(
      (z, j) => z > i * 0.2 + 1e-6 && (zs[j - 1] ?? 0) < (i + 1) * 0.2 - 1e-6,
    );
    return model
      .filter((s) => included.includes(s.to[2]))
      .map((s) => `M${s.from[0]},${s.from[1]}L${s.to[0]},${s.to[1]}`)
      .join("");
  });
}
