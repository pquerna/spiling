# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
import unittest
from loop import verify_source, rows, grade, overlay_algorithm, workspace_manifest, sync_source, editable, ROUTING
from pathlib import Path
import tempfile
import tomllib


class FrozenJudgeTests(unittest.TestCase):
    def test_workspace_manifest_keeps_only_the_frozen_closure(self):
        original='[workspace]\nresolver = "3"\nmembers = ["a", "crates/deposition-routing"]\ndefault-members = ["a"]\nexclude = ["v"]\n\n[workspace.package]\nversion = "0.1.0"\n'
        result=tomllib.loads(workspace_manifest(original))['workspace']
        self.assertEqual(result['members'],['crates/deposition-routing','crates/compute','crates/contracts'])
        self.assertNotIn('default-members',result);self.assertNotIn('exclude',result)
        self.assertEqual(result['package']['version'],'0.1.0')

    def test_only_algorithm_may_change(self):
        r=ROUTING
        original={r+'src/search.rs':'old',r+'examples/matrix/judge.rs':'judge','crates/compute/src/lib.rs':'compute'}
        verify_source(original,{**original,r+'src/search.rs':'new',r+'src/search/beam.rs':'helper'})
        verify_source({**original,r+'src/search/old.rs':'old'},original)
        for name in [r+'Cargo.toml',r+'src/search/config.json',r+'src/travel.rs',r+'src/search.py','crates/compute/src/lib.rs',
                     'crates/compute/src/search.rs','repository:Cargo.lock','Cargo.toml']:
            with self.assertRaises(ValueError):verify_source(original,{**original,name:'changed'})
        with self.assertRaisesRegex(ValueError,'frozen'):
            verify_source(original,{**original,r+'examples/matrix/judge.rs':'weaker'})
        with self.assertRaises(ValueError):verify_source(original,{r+'src/search.rs':'new'})
        self.assertFalse(editable('crates/compute/src/search.rs'))

    def test_invalid_incomplete_and_nonfinite_matrix_cannot_score(self):
        header='case\tsplit\talgorithm\tstatus\ttravel_mm\tstarts\tdeposition_mm\tlongest_chain_mm\twaypoints\tmotion_lower_bound_s\tmachine_time_s\tstarts_bound\ttravel_bound_mm\twork\tplanning_us\n'
        row='one\tdiscovery\tbaseline\tok\t10\t1\t5\t5\t0\t1\t2\t1\t4\t10\t10\n'
        with self.assertRaisesRegex(ValueError,'unpaired'):rows(header+row)
        with self.assertRaises(ValueError):rows(header+row+row)
        with self.assertRaisesRegex(ValueError,'finite'):rows(header+row.replace('\t10\t1\t','\tnan\t1\t'))
        with self.assertRaisesRegex(ValueError,'validity'):rows(header+row.replace('\tok\t','\tfailed\t'))

    def test_zero_travel_remains_valid_and_is_not_hidden_by_a_pseudocount(self):
        panel=[]
        for split in ['discovery','holdout']:
            for algorithm,cost in [('baseline',0.),('candidate',0.)]:
                panel.append(dict(case=split,split=split,algorithm=algorithm,status='ok',travel_mm=cost,starts=1.,motion_lower_bound_s=1.,machine_time_s=1.,starts_bound=1.,travel_bound_mm=0.))
        self.assertEqual(grade(panel)[-1]['metrics']['travel_mm']['geometric_mean_ratio'],1.)
        panel[-1]['travel_mm']=1.
        result=grade(panel)[-1]['metrics']['travel_mm']
        self.assertFalse(result['relative_comparison_complete'])
        self.assertIsNone(result['geometric_mean_ratio'])

    def test_sync_writes_changed_bytes_and_preserves_unchanged_files(self):
        import os
        with tempfile.TemporaryDirectory() as temporary:
            source=Path(temporary)/'source';target=Path(temporary)/'build';source.mkdir()
            file=source/'search.rs';file.write_text('one');os.utime(file,(1,1))
            sync_source(source,target)
            first=(target/'search.rs').stat().st_mtime_ns
            self.assertGreater(first,file.stat().st_mtime_ns)
            sync_source(source,target)
            self.assertEqual((target/'search.rs').stat().st_mtime_ns,first)
            file.write_text('two');os.utime(file,(1,1))
            (target/'stale.rs').write_text('removed module')
            sync_source(source,target)
            self.assertEqual((target/'search.rs').read_text(),'two')
            self.assertNotEqual((target/'search.rs').stat().st_mtime_ns,file.stat().st_mtime_ns)
            self.assertFalse((target/'stale.rs').exists())

    def test_overlay_replaces_helper_inventory(self):
        with tempfile.TemporaryDirectory() as temporary:
            source=Path(temporary)/'source';target=Path(temporary)/'target'
            r=ROUTING
            for root in (source,target):(root/r/'src/search').mkdir(parents=True)
            (source/r/'src/search.rs').write_text('new')
            (source/r/'src/search/beam.rs').write_text('beam')
            (target/r/'src/search/stale.rs').write_text('stale')
            (target/r/'src/lib.rs').write_text('protected')
            overlay_algorithm(source,target)
            self.assertFalse((target/r/'src/search/stale.rs').exists())
            self.assertEqual((target/r/'src/search/beam.rs').read_text(),'beam')
            self.assertEqual((target/r/'src/lib.rs').read_text(),'protected')

    def test_failed_complete_census_is_retained_but_never_scored(self):
        header='case\tsplit\talgorithm\tstatus\ttravel_mm\tstarts\tdeposition_mm\tlongest_chain_mm\twaypoints\tmotion_lower_bound_s\tmachine_time_s\tstarts_bound\ttravel_bound_mm\twork\tplanning_us\n'
        text=header+'one\tdiscovery\tbaseline\tok\t10\t1\t5\t5\t0\t1\t2\t1\t4\t10\t10\n'+'one\tdiscovery\tcandidate\tno_route\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t10\n'
        parsed=rows(text,require_valid=False,expected=[('one','discovery')])
        self.assertEqual(len(parsed),2)
        with self.assertRaisesRegex(ValueError,'validity'):grade(parsed)
        with self.assertRaisesRegex(ValueError,'omitted'):rows(text,require_valid=False,expected=[('one','discovery'),('two','holdout')])

    def test_directional_case_regression_remains_visible(self):
        panel=[]
        for name,split,travel in [('a','discovery',5.),('b','holdout',12.)]:
            for algorithm,cost in [('baseline',10.),('candidate',travel)]:
                panel.append(dict(case=name,split=split,algorithm=algorithm,status='ok',travel_mm=cost,starts=1.,motion_lower_bound_s=cost,machine_time_s=cost,starts_bound=1.,travel_bound_mm=4.))
        result=grade(panel)
        self.assertLess(result[-1]['metrics']['travel_mm']['geometric_mean_ratio'],1.)
        self.assertEqual(result[-1]['metrics']['travel_mm']['worst_ratio'],1.2)
        self.assertEqual(result[1]['metrics']['travel_mm']['per_case'],{'b':1.2})
        self.assertAlmostEqual(result[1]['gap_to_bound']['travel_mm'],3.)
        self.assertEqual(result[-1]['gap_to_bound']['starts_at_bound'],2)

    def test_route_below_a_lower_bound_is_a_harness_fault_not_a_score(self):
        header='case\tsplit\talgorithm\tstatus\ttravel_mm\tstarts\tdeposition_mm\tlongest_chain_mm\twaypoints\tmotion_lower_bound_s\tmachine_time_s\tstarts_bound\ttravel_bound_mm\twork\tplanning_us\n'
        row='one\tdiscovery\t%s\tok\t10\t1\t5\t5\t0\t1\t2\t%s\t%s\t10\t10\n'
        rows(header+row%('baseline','1','4')+row%('candidate','1','10'))
        for starts,travel in [('2','4'),('1','11')]:
            with self.assertRaisesRegex(ValueError,'bound'):rows(header+row%('baseline','1','4')+row%('candidate',starts,travel))


if __name__=='__main__':unittest.main()
