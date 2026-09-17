import tempfile
import unittest
from pathlib import Path

from ci.trace_profiler import analyze_trace_log, generate_summary_markdown


def analyze(lines):
    with tempfile.NamedTemporaryFile("w", suffix=".log", delete=False) as log:
        log.write("\n".join(lines))
        path = Path(log.name)
    try:
        return analyze_trace_log(str(path))
    finally:
        path.unlink()


class E2EAccountingTests(unittest.TestCase):
    def test_legacy_recursion_remains_sequential(self):
        result = analyze([
            'block_number: 23817600',
            'reth-block [ 36.5s | 100.00% ] block_number: 23817600',
            'host.execute [ 70.6ms | 0.19% ]',
            'ceno prove-stark app create_proof time: 27.461683669s',
            'ceno prove-stark recursion setup time (gpu): 3.326182457s',
            'ceno prove-stark recursion total create_proof time (gpu): 6.146067834s',
            'ceno prove-stark root verify time: 27.69658ms',
            'ceno prove-stark total create_proof time (gpu): 33.674897227s',
            'recursion.compress_to_root_proof [ 8.99s | 100.00% ]',
            'app_prove.inner [ 25.0s | 75.00% ]',
        ])
        stats = result[4]

        self.assertAlmostEqual(result[0], 25.0)
        self.assertAlmostEqual(stats['app_prove_time'], 27.461683669)
        self.assertEqual(stats['recursion_mode'], 'sequential')
        self.assertAlmostEqual(stats['recursion_tail_time'], 6.146067834)
        self.assertEqual(stats['recursion_overlap_time'], 0.0)
        self.assertAlmostEqual(stats['create_proof_unattributed_time'], 0.067145724)

        markdown = generate_summary_markdown(result[0], result[1], result[2], stats, result[5], result[6])
        self.assertIn('| recursion_sequential | 6.146 |', markdown)
        self.assertNotIn('| recursion_overlap |', markdown)
        self.assertIn('| total_create_proof | 33.675 |', markdown)

    def test_streaming_uses_dual_worker_critical_lifetime(self):
        result = analyze([
            'block_number: 23817600',
            'reth-block [ 37.4s | 100.00% ] block_number: 23817600',
            'host.execute [ 70.8ms | 0.19% ]',
            'ceno prove-stark app create_proof time: 33.959403816s',
            'streaming recursion worker complete | device_id: 0 | total_ms: 3744 | phase: "recursion_worker_metrics"',
            'streaming recursion worker complete | device_id: 1 | total_ms: 5271 | phase: "recursion_worker_metrics"',
            'ceno prove-stark recursion streaming time (gpu): 36.78769798s',
            'ceno prove-stark root verify time: 27.743014ms',
            'ceno prove-stark total create_proof time (gpu): 36.78769897s',
        ])
        stats = result[4]

        self.assertEqual(stats['recursion_mode'], 'streaming')
        self.assertAlmostEqual(stats['recursion_streaming_time'], 36.78769798)
        self.assertAlmostEqual(stats['recursion_worker_critical_time'], 5.271)
        self.assertAlmostEqual(stats['recursion_tail_time'], 2.828295154)
        self.assertAlmostEqual(stats['recursion_overlap_time'], 2.442704846)
        self.assertEqual(stats['recursion_time'], stats['recursion_tail_time'])
        self.assertAlmostEqual(stats['host_executor_time'], 0.0708)

        markdown = generate_summary_markdown(result[0], result[1], result[2], stats, result[5], result[6])
        self.assertIn('| **reth_block_total** | **37.400** | **100.00%** |', markdown)
        self.assertIn('| recursion_tail | 2.828 |', markdown)
        self.assertIn('| recursion_worker_critical | 5.271 |', markdown)
        self.assertIn('| recursion_overlap | 2.443 |', markdown)
        self.assertIn('| app_prove_interval | 33.959 |', markdown)
        self.assertNotIn('| base_proving |', markdown)
        self.assertNotIn('| recursion | 36.788 |', markdown)

    def test_streaming_single_worker_lifetime(self):
        result = analyze([
            'block_number: 23587691',
            'emulator.preflight_execute [ 1.500s | 10.00% ]',
            'ceno prove-stark app create_proof time: 15.500s',
            'streaming recursion worker complete | device_id: 0 | total_ms: 2538 | phase: "recursion_worker_metrics"',
            'ceno prove-stark recursion streaming time (gpu): 18.000s',
            'ceno prove-stark total create_proof time (gpu): 18.205s',
        ])
        stats = result[4]

        self.assertEqual(stats['recursion_mode'], 'streaming')
        self.assertAlmostEqual(stats['recursion_worker_critical_time'], 2.538)
        self.assertAlmostEqual(stats['recursion_tail_time'], 2.705)
        self.assertEqual(stats['recursion_overlap_time'], 0.0)
        self.assertAlmostEqual(stats['reth_block_time'], 18.205)
        self.assertTrue(stats['reth_block_time_synthesized'])


if __name__ == '__main__':
    unittest.main()
