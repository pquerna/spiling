# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
import unittest
from loop import verify_source, rows, grade, standalone_manifest
import tomllib


class FrozenJudgeTests(unittest.TestCase):
    def test_standalone_manifest_preserves_distinct_package_and_rust_versions(self):
        original='[package]\nversion.workspace = true\nrust-version.workspace = true\nedition.workspace = true\nlicense.workspace = true\nrepository.workspace = true\n'
        result=standalone_manifest(original,{'version':'0.1.0','rust-version':'1.99','edition':'2024','license':'OSL-3.0','repository':'example'})
        package=tomllib.loads(result)['package']
        self.assertEqual(package['version'],'0.1.0')
        self.assertEqual(package['rust-version'],'1.99')

    def test_only_algorithm_may_change(self):
        original={'src/search.rs':'old','examples/matrix/judge.rs':'judge'}
        verify_source(original,{**original,'src/search.rs':'new'})
        with self.assertRaisesRegex(ValueError,'frozen'):
            verify_source(original,{**original,'examples/matrix/judge.rs':'weaker'})
        with self.assertRaises(ValueError):verify_source(original,{'src/search.rs':'new'})

    def test_invalid_incomplete_and_nonfinite_matrix_cannot_score(self):
        header='case\tsplit\talgorithm\tstatus\ttravel_mm\tstarts\tdeposition_mm\tlongest_chain_mm\twaypoints\tmotion_lower_bound_s\twork\tplanning_us\n'
        row='one\tdiscovery\tbaseline\tok\t10\t1\t5\t5\t0\t1\t10\t10\n'
        with self.assertRaisesRegex(ValueError,'unpaired'):rows(header+row)
        with self.assertRaises(ValueError):rows(header+row+row)
        with self.assertRaisesRegex(ValueError,'finite'):rows(header+row.replace('\t10\t1\t','\tnan\t1\t'))
        with self.assertRaisesRegex(ValueError,'validity'):rows(header+row.replace('\tok\t','\tfailed\t'))

    def test_zero_travel_remains_valid_and_is_not_hidden_by_a_pseudocount(self):
        panel=[]
        for split in ['discovery','holdout']:
            for algorithm,cost in [('baseline',0.),('candidate',0.)]:
                panel.append(dict(case=split,split=split,algorithm=algorithm,status='ok',travel_mm=cost,starts=1.,motion_lower_bound_s=1.))
        self.assertEqual(grade(panel)[-1]['metrics']['travel_mm']['geometric_mean_ratio'],1.)
        panel[-1]['travel_mm']=1.
        result=grade(panel)[-1]['metrics']['travel_mm']
        self.assertFalse(result['relative_comparison_complete'])
        self.assertIsNone(result['geometric_mean_ratio'])

    def test_directional_case_regression_remains_visible(self):
        panel=[]
        for name,split,travel in [('a','discovery',5.),('b','holdout',12.)]:
            for algorithm,cost in [('baseline',10.),('candidate',travel)]:
                panel.append(dict(case=name,split=split,algorithm=algorithm,status='ok',travel_mm=cost,starts=1.,motion_lower_bound_s=cost))
        result=grade(panel)
        self.assertLess(result[-1]['metrics']['travel_mm']['geometric_mean_ratio'],1.)
        self.assertEqual(result[-1]['metrics']['travel_mm']['worst_ratio'],1.2)
        self.assertEqual(result[1]['metrics']['travel_mm']['per_case'],{'b':1.2})


if __name__=='__main__':unittest.main()
