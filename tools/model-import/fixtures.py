# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0

"""Generate original mesh-import acceptance sources; no third-party geometry."""
import math,struct,zipfile,json
from pathlib import Path


def stl(root,name,p,t):
 data=b'solid original MS1 fixture'.ljust(80,b'\0')+struct.pack('<I',len(t))
 for tri in t:data+=struct.pack('<12fH',0,0,0,*(c for v in tri for c in p[v]),0)
 (root/name).write_bytes(data)
 return p,t

def bore(root):
 n=128;p=[]
 for z in [0,8]:
  for radius in [8,2]:
   for i in range(n):
    a=math.tau*i/n;x,y=math.cos(a),math.sin(a)
    r=radius/max(abs(x),abs(y)) if radius==8 else radius
    p.append((r*x,r*y,z))
 t=[]
 for i in range(n):
  j=(i+1)%n
  t.extend([(i,j,2*n+j),(i,2*n+j,2*n+i),(n+i,3*n+j,n+j),(n+i,3*n+i,3*n+j),
   (i,n+j,j),(i,n+i,n+j),(2*n+i,2*n+j,3*n+j),(2*n+i,3*n+j,3*n+i)])
 return stl(root,'bore.stl',p,t)

def cantilever(root):
 cells={(x,y,z) for x in range(4) for y in range(2) for z in range(3) if x==0 or z==2}
 p=[];t=[];vertices={}
 def idx(v):
  if v not in vertices:vertices[v]=len(p);p.append(tuple(4*c for c in v))
  return vertices[v]
 faces=[((-1,0,0),[(0,0,0),(0,0,1),(0,1,1),(0,1,0)]),((1,0,0),[(1,0,0),(1,1,0),(1,1,1),(1,0,1)]),
 ((0,-1,0),[(0,0,0),(1,0,0),(1,0,1),(0,0,1)]),((0,1,0),[(0,1,0),(0,1,1),(1,1,1),(1,1,0)]),
 ((0,0,-1),[(0,0,0),(0,1,0),(1,1,0),(1,0,0)]),((0,0,1),[(0,0,1),(1,0,1),(1,1,1),(0,1,1)])]
 for cell in sorted(cells):
  for delta,quad in faces:
   if tuple(cell[a]+delta[a] for a in range(3)) in cells:continue
   q=[idx(tuple(cell[a]+v[a] for a in range(3))) for v in quad]
   t.extend([(q[0],q[1],q[2]),(q[0],q[2],q[3])])
 return stl(root,'cantilever.stl',p,t)

def mf(root,p,t):
 rels='<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="r1" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel" Target="/3D/root.model"/></Relationships>'
 root_model='<model xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02" xmlns:p="http://schemas.microsoft.com/3dmanufacturing/production/2015/06" requiredextensions="p"><resources/><build><item objectid="1" p:path="/3D/Objects/body.model" transform="1 0 0 0 1 0 0 0 1 30 30 0"/><item objectid="1" p:path="/3D/Objects/body.model" transform="-1 0 0 0 1 0 0 0 1 60 30 0"/><item objectid="1" p:path="/3D/Objects/body.model" transform="1 0 0 0 1 0 0 0 1 90 30 0"/></build></model>'
 sub='<model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"><resources><object id="1"><components><component objectid="2"/></components></object><object id="2" name="Original bored plate"><mesh><vertices>'
 sub+=''.join(f'<vertex x="{x}" y="{y}" z="{z}"/>' for x,y,z in p)+'</vertices><triangles>'
 sub+=''.join(f'<triangle v1="{a}" v2="{b}" v3="{c}"/>' for a,b,c in t)+'</triangles></mesh></object></resources></model>'
 with zipfile.ZipFile(root/'assembly.3mf','w',compression=zipfile.ZIP_DEFLATED) as z:
  for name,data in [('_rels/.rels',rels),('3D/root.model',root_model),('3D/Objects/body.model',sub),('[Content_Types].xml','<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/></Types>')]:z.writestr(name,data)

def generate(root):
 root.mkdir(parents=True, exist_ok=True)
 p,t=bore(root);mf(root,p,t);cantilever(root)
 p=[(0,0,0),(12,0,0),(12,12,0),(0,12,0),(0,0,6),(12,0,6),(12,12,6),(0,12,6)]
 t=[(0,2,1),(0,3,2),(4,5,6),(4,6,7),(0,1,5),(0,5,4),(1,2,6),(1,6,5),(2,3,7),(2,7,6),(3,0,4),(3,4,7)]
 stl(root,'box.stl',p,t)

if __name__=='__main__':
 import argparse
 parser=argparse.ArgumentParser();parser.add_argument('destination',type=Path)
 generate(parser.parse_args().destination)
