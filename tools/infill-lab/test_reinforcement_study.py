# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Qualification cannot hide failed refinement or change physical input."""
import json
from pathlib import Path
from types import SimpleNamespace

import pytest

from campaign import write
from model import Material, digest, file_hash
from reinforcement_study import axial_reference, collect, read_row
from cathedral import CASES


def test_axial_reference_is_exact_only_for_a_constant_actual_section():
    road = dict(start=[-1.,0.,.2], end=[5.,0.,.2], width=.4,
                lower=0., upper=.2, kind=5, owner=0)
    data = dict(roads=[road, dict(road, start=[-1.,0.,.4], end=[5.,0.,.4], lower=.2, upper=.4)])
    bounds = [[0.,-.5,0.],[4.,.5,.4]]
    ref = axial_reference(data, bounds, Material(z_ratio=.25))
    assert ref['admitted']
    assert ref['stiffness_n_mm'] == pytest.approx(2300*.25*4*.4/.4)
    data['roads'][1]['width'] = .3
    assert axial_reference(data, bounds, Material(z_ratio=.25)) == dict(admitted=False, reason='changing_material_section')


def fixture(tmp_path):
    inputs, outputs = tmp_path/'inputs', tmp_path/'results'
    inputs.mkdir(); outputs.mkdir(); jobs=[]
    scenario = dict(bounds=[[0,0,0],[1,1,1]])
    for variant in ['control','candidate']:
        for h in [2.,1.]:
            for case in CASES:
                physical = dict(scenario, case=case, refinement=dict(max_xy=h))
                roads = '{"roads": []}\n'
                folder = inputs/f'pending-{len(jobs)}'; folder.mkdir(); (folder/'roads.json').write_text(roads)
                spec = dict(variant=variant, scenarios=[physical], judge={}, roads_sha256=file_hash(folder/'roads.json'))
                key=digest(spec); folder.rename(inputs/key); folder=inputs/key
                write(folder/'case.json',spec)
                job=dict(id=key,specimen='cube48',variant=variant,case=case,max_xy=h,scenario=physical)
                jobs.append(job)
                result=outputs/key; result.mkdir()
                write(result/'launch.json',dict(removed=True,backend='reference',image=dict(id='frozen'),docker_exit=0))
                row=dict(status='ok',input_sha256=file_hash(folder/'case.json'),judge={},scenario=physical,
                         stiffness_n_mm=100. if variant=='control' else 110.,
                         components=[dict(force_balance=0.,energy_error=0.,volume_relative_error=0.)])
                (result/'trials.jsonl').write_text(json.dumps(row)+'\n')
    write(inputs/'jobs.json',dict(jobs=jobs,backend='reference',max_xy=[2.,1.],specimens=['cube48'],costs=[],
                                 analytical=[dict(specimen='cube48',variant=v,admitted=False) for v in ['control','candidate']],scope='finite_test'))
    image=tmp_path/'image.json'; write(image,dict(id='frozen'))
    return SimpleNamespace(inputs=inputs,results=outputs,image=image,output=tmp_path/'report.json'),jobs


def test_failed_finest_solve_remains_unknown_instead_of_selecting_coarse(tmp_path):
    args,jobs=fixture(tmp_path)
    collect(args)
    report=json.loads(args.output.read_text())
    assert report['numerical_gate_passed']
    assert not report['independent_formulation_qualified']
    assert report['comparisons'][0]['provisional']
    fine=next(j for j in jobs if j['variant']=='candidate' and j['max_xy']==1.)
    (args.results/fine['id']/'trials.jsonl').unlink()
    collect(args)
    report=json.loads(args.output.read_text())
    assert not report['numerical_gate_passed']
    assert report['comparisons'][0]['ratios'] is None
    assert any(r['status']=='unknown' for r in report['rows'])


def test_changed_roads_and_changed_scenario_cannot_supply_a_response(tmp_path):
    args,jobs=fixture(tmp_path); job=jobs[0]
    folder=args.inputs/job['id']; original=(folder/'roads.json').read_text()
    (folder/'roads.json').write_text('changed')
    with pytest.raises(ValueError, match='road bytes'):
        read_row(args.inputs,args.results,job,'reference','frozen')
    (folder/'roads.json').write_text(original)
    row=json.loads((args.results/job['id']/'trials.jsonl').read_text())
    row['scenario']['bounds'][1][0]=2
    (args.results/job['id']/'trials.jsonl').write_text(json.dumps(row)+'\n')
    with pytest.raises(ValueError, match='scenario'):
        read_row(args.inputs,args.results,job,'reference','frozen')


def test_independent_agreement_requires_every_duty_and_identical_native_physics():
    from reinforcement_study import compare_formulations
    def panel(element):
        return dict(numerical_gate_passed=True,
                    costs=[dict(specimen='cube48',variant=v,source_sha256='source',roads_sha256=v,binaries={'engine':'same'}) for v in ['control','candidate']],
                    rows=[dict(specimen='cube48',variant=v,case=c,max_xy=1.,status='ok',image='frozen',stiffness_n_mm=100.,
                               scenario=dict(element=element,bounds=[[0,0,0],[1,1,1]],case=c,material={'z_ratio':.25},refinement={}))
                          for v in ['control','candidate'] for c in CASES])
    first,second=panel('C3D10'),panel('C3D15')
    assert compare_formulations(first,second)['independent_formulation_qualified']
    second['rows'][0]['stiffness_n_mm']=90.
    assert not compare_formulations(first,second)['independent_formulation_qualified']
    second=panel('C3D15');second['costs'][0]['roads_sha256']='different'
    with pytest.raises(ValueError,match='native captures'):
        compare_formulations(first,second)
    second=panel('C3D15');second['rows'][0]['scenario']['bounds'][1][0]=2
    with pytest.raises(ValueError,match='physical'):
        compare_formulations(first,second)
    second=panel('C3D15');second['numerical_gate_passed']=False
    assert not compare_formulations(first,second)['independent_formulation_qualified']
