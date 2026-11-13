/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
// Original rectangular-void witness. Independent emitted-command decoder;
// no plan, source sections or role hints decide whether material enters the cut.
import fs from 'node:fs';
import path from 'node:path';
import { decodeGcode } from '../gcode-parity/decode.mjs';
const root=path.resolve(process.argv[2]);
const lower=[108.6,108.6,1.2],upper=[111.4,111.4,4.8];
function crosses(a,b) {
  let lo=0,hi=1;
  for(let i=0;i<3;i++) {
    const d=b[i]-a[i];
    if(d===0) {if(a[i]<=lower[i] || a[i]>=upper[i])return false;continue;}
    const t1=(lower[i]-a[i])/d,t2=(upper[i]-a[i])/d;
    lo=Math.max(lo,Math.min(t1,t2));hi=Math.min(hi,Math.max(t1,t2));
    if(lo>=hi)return false;
  }
  return hi>lo;
}
const rows=[];
for(const name of ['box','negative-prusa','negative-bambu','nested']) {
  const decoded=decodeGcode(fs.readFileSync(path.join(root,name,'export/program.gcode'),'utf8'));
  const inside=decoded.strokes.filter(s=>crosses(s.from,s.to)).length;
  if((name==='box' && inside===0) || (name!=='box' && inside!==0))throw Error(`${name}: ${inside} depositing segments enter the controlled inset`);
  rows.push({case:name,depositionSegments:decoded.strokes.length,segmentsInInsetVoid:inside});
}
const report={scope:'Independent full-segment intersection with the original inset rectangular void; current G0/G1 witnesses. The shared observer refuses unsupported commands.',rows};
fs.writeFileSync(path.join(root,'void-witness.json'),JSON.stringify(report,null,2)+'\n');
console.log(JSON.stringify(report));
