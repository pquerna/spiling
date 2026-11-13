# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0

"""Generate original mesh-import acceptance sources; no third-party geometry."""
import math,struct,zipfile,json
from pathlib import Path


def stl(root,name,p,t):
 with (root/name).open('wb') as stream:
  stream.write(b'solid original MS1 fixture'.ljust(80,b'\0')+struct.pack('<I',len(t)))
  for tri in t:stream.write(struct.pack('<12fH',0,0,0,*(c for v in tri for c in p[v]),0))
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



def compatibility(root):
 """Focused material/repair controls; original geometry and metadata."""
 p=[(0,0,0),(12,0,0),(12,12,0),(0,12,0),(0,0,6),(12,0,6),(12,12,6),(0,12,6)]
 t=[(0,2,1),(0,3,2),(4,5,6),(4,6,7),(0,1,5),(0,5,4),(1,2,6),(1,6,5),(2,3,7),(2,7,6),(3,0,4),(3,4,7)]
 stl(root,'debris.stl',p+[(1000,1000,1000)],t+[(8,8,8)])
 stl(root,'crack.stl',p+[(0,0,.00001)],[(8,2,1),*t[1:]])
 stl(root,'nested.stl',p+[(4+x/3,4+y/3,1+z*2/3) for x,y,z in p],t+[tuple(v+8 for v in a[::-1]) for a in t])
 core='http://schemas.microsoft.com/3dmanufacturing/core/2015/02'
 rels='<Relationships><Relationship Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel" Target="/3D/root.model"/></Relationships>'
 def mesh(points,triangles):
  return '<mesh><vertices>'+''.join(f'<vertex x="{x}" y="{y}" z="{z}"/>' for x,y,z in points)+'</vertices><triangles>'+''.join(f'<triangle v1="{a}" v2="{b}" v3="{c}"/>' for a,b,c in triangles)+'</triangles></mesh>'
 def obj(id,points=p,triangles=t):return f'<object id="{id}" name="Part {id}">'+mesh(points,triangles)+'</object>'
 def write(name,resources,items='<item objectid="1"/>',metadata=None,attrs=''):
  with zipfile.ZipFile(root/name,'w',zipfile.ZIP_DEFLATED) as z:
   z.writestr('_rels/.rels',rels);z.writestr('3D/root.model',f'<model xmlns="{core}" {attrs}><resources>{resources}</resources><build>{items}</build></model>')
   for key,value in (metadata or {}).items():z.writestr(key,value)
 q=[(4+x/3,4+y/3,z) for x,y,z in p]
 prusa='<config><object id="1"><volume firstid="0" lastid="11"><metadata key="volume_type" value="ModelPart"/></volume><volume firstid="12" lastid="23"><metadata key="volume_type" value="NegativeVolume"/></volume></object></config>'
 write('negative-prusa.3mf',obj(1,p+q,t+[tuple(v+8 for v in a) for a in t]),metadata={'Metadata/Slic3r_PE_model.config':prusa})
 assembly='<object id="3"><components><component objectid="1"/><component objectid="2"/></components></object>'
 bambu='<config><object id="3"><part id="1" subtype="normal_part"/><part id="2" subtype="negative_part"/></object></config>'
 resources=obj(1)+obj(2,q)+assembly
 write('negative-bambu.3mf',resources,'<item objectid="3"/>',{'Metadata/model_settings.config':bambu})
 write('negative-scoped.3mf',resources,'<item objectid="3"/><item objectid="1" transform="1 0 0 0 1 0 0 0 1 20 0 0"/>',{'Metadata/model_settings.config':bambu})
 write('modifier.3mf',resources,'<item objectid="3"/>',{'Metadata/model_settings.config':bambu.replace('negative_part','modifier_part')})
 write('cura-namespace.3mf',obj(1),attrs='xmlns:cura="http://software.ultimaker.com/xml/cura/3mf/2015/10"')
 plates='<config><plate><metadata key="plater_id" value="1"/><model_instance><metadata key="object_id" value="1"/><metadata key="instance_id" value="0"/></model_instance></plate><plate><metadata key="plater_id" value="2"/><model_instance><metadata key="object_id" value="1"/><metadata key="instance_id" value="1"/></model_instance></plate></config>'
 write('plates.3mf',obj(1),'<item objectid="1"/><item objectid="1" transform="1 0 0 0 1 0 0 0 1 20 0 0"/>',{'Metadata/model_settings.config':plates})


def scale(root,segments=262144,instances=200):
 """Original source-load workloads, not fixed time/space acceptance gates."""
 root.mkdir(parents=True,exist_ok=True)
 p=[(20*math.cos(math.tau*i/segments),20*math.sin(math.tau*i/segments),z) for z in (0.,20.) for i in range(segments)]
 p.extend([(0.,0.,0.),(0.,0.,20.)])
 def triangles():
  for i in range(segments):
   j=(i+1)%segments
   yield from [(i,j,segments+j),(i,segments+j,segments+i),(2*segments,j,i),(2*segments+1,segments+i,segments+j)]
 with (root/'large-cylinder.stl').open('wb') as stream:
  stream.write(b'solid original import scale'.ljust(80,b'\0')+struct.pack('<I',4*segments))
  for t in triangles():stream.write(struct.pack('<12fH',0,0,0,*(c for v in t for c in p[v]),0))
 compatibility(root)
 with zipfile.ZipFile(root/'cura-namespace.3mf') as z:entries={n:z.read(n) for n in z.namelist()}
 model=entries['3D/root.model'].decode();a=model.index('<build>');b=model.index('</build>',a)
 model=model[:a]+'<build>'+''.join(f'<item objectid="1" transform="1 0 0 0 1 0 0 0 1 {20*(i%20)} {20*(i//20)} 0"/>' for i in range(instances))+'</build>'+model[b+8:]
 entries['3D/root.model']=model.encode()
 with zipfile.ZipFile(root/'instances-200.3mf','w',zipfile.ZIP_DEFLATED) as z:
  for name,data in entries.items():z.writestr(name,data)

def torus(root,rings=1024,sides=512):
 """Curved, through-hole holdout: 1,048,576 facets without cap fans."""
 root.mkdir(parents=True,exist_ok=True)
 p=[((20+5*math.cos(math.tau*j/sides))*math.cos(math.tau*i/rings),
     (20+5*math.cos(math.tau*j/sides))*math.sin(math.tau*i/rings),
     5+5*math.sin(math.tau*j/sides)) for i in range(rings) for j in range(sides)]
 with (root/'large-torus.stl').open('wb') as stream:
  stream.write(b'solid original curved import scale'.ljust(80,b'\0')+struct.pack('<I',2*rings*sides))
  for i in range(rings):
   for j in range(sides):
    a=i*sides+j;b=((i+1)%rings)*sides+j
    c=((i+1)%rings)*sides+(j+1)%sides;d=i*sides+(j+1)%sides
    for t in [(a,b,c),(a,c,d)]:
     stream.write(struct.pack('<12fH',0,0,0,*(v for k in t for v in p[k]),0))

if __name__=='__main__':
 import argparse
 parser=argparse.ArgumentParser();parser.add_argument('destination',type=Path);parser.add_argument('--scale',action='store_true');parser.add_argument('--torus',action='store_true')
 options=parser.parse_args()
 if options.torus:torus(options.destination)
 elif options.scale:scale(options.destination)
 else:generate(options.destination)
