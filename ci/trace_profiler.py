#!/usr/bin/env python3
"""
Analyze trace log and generate breakdown statistics in Markdown format.
"""

import re
from collections import defaultdict
from typing import Dict, Tuple, List


def parse_time_to_seconds(time_str: str) -> float:
    """Convert time string (e.g., '22.8s', '100ms', '1.5µs', '80.0ns') to seconds."""
    time_str = time_str.strip()
    if time_str.endswith('ns'):
        return float(time_str[:-2]) / 1_000_000_000
    elif time_str.endswith('µs'):
        return float(time_str[:-2]) / 1_000_000
    elif time_str.endswith('ms'):
        return float(time_str[:-2]) / 1000
    elif time_str.endswith('s'):
        return float(time_str[:-1])
    return 0.0


def get_indent_level(line: str) -> int:
    """Get the indentation level of a line (count tree branch characters and spaces)."""
    # Remove ANSI color codes first
    clean_line = re.sub(r'\[1;32m|\[0m', '', line)
    # Find the position of the last tree character (│, ┝, ┕, etc.)
    # The indent level is based on where content starts (after tree chars and spaces)
    match = re.search(r'[│┝┕┗┣┏┓┛].*?[┝┕┗┣━]\s*', clean_line)
    if match:
        # Return the end position of the tree structure as indent level
        return match.end()
    # Fallback: count pipes
    return clean_line.count('│')


def parse_trace_line(line: str) -> Tuple[str, float, str, int]:
    """
    Parse a trace log line and return (operation_name, time_in_seconds, extra_info, indent_level).
    """
    # Match pattern: [ceno] operation_name [ time | ... ] optional_extra
    # The [ceno] prefix is optional
    # Support hyphens, dots, and underscores in operation names
    pattern = r'(?:\[ceno\]\s+)?([\w.-]+)\s*\[\s*([0-9.]+(?:ns|ms|µs|s))\s*\|'
    match = re.search(pattern, line)

    if match:
        operation = match.group(1)
        time_str = match.group(2)
        time_seconds = parse_time_to_seconds(time_str)
        indent = get_indent_level(line)

        # Extract extra info like name or shard_id
        extra_info = ""
        if 'name:' in line:
            name_match = re.search(r'name:\s*"([^"]+)"', line)
            if name_match:
                extra_info = name_match.group(1)
        elif 'table_name:' in line:
            table_match = re.search(r'table_name:\s*"([^"]+)"', line)
            if table_match:
                extra_info = table_match.group(1)
        elif 'shard_id:' in line:
            shard_match = re.search(r'shard_id:\s*(\d+)', line)
            if shard_match:
                extra_info = f"shard_{shard_match.group(1)}"
        elif 'block_number:' in line:
            block_match = re.search(r'block_number:\s*(\d+)', line)
            if block_match:
                extra_info = f"block_{block_match.group(1)}"

        return operation, time_seconds, extra_info, indent

    return "", 0.0, "", 0


def analyze_trace_log(file_path: str):
    """Analyze the trace log file and generate statistics."""

    # Storage for statistics
    total_by_operation = defaultdict(float)
    chip_proof_by_table = defaultdict(float)
    chip_operations = defaultdict(lambda: defaultdict(float))  # chip_name -> {operation -> time}
    app_prove_inner_time = 0.0

    # Shard-level statistics for overlap analysis
    generate_witness_by_shard = {}  # shard_id -> time
    create_proof_by_shard = {}  # shard_id -> time

    # Module-level statistics for GPU operations breakdown
    module_operations = defaultdict(lambda: defaultdict(float))  # module_name -> {gpu_operation -> time}

    # E2E layer statistics
    e2e_stats = {
        'reth_block_time': 0.0,
        'block_number': '',
        'emulator_time': 0.0,
        'app_prove_time': 0.0,
        'recursion_time': 0.0,
    }

    # Read and parse the file
    with open(file_path, 'r', encoding='utf-8') as f:
        lines = f.readlines()

    # First pass: find key timings
    for line in lines:
        # Find reth-block (E2E time)
        if 'reth-block' in line or 'reth_block' in line:
            operation, time_seconds, extra_info, _ = parse_trace_line(line)
            if operation in ['reth-block', 'reth_block']:
                e2e_stats['reth_block_time'] = time_seconds
                if extra_info.startswith('block_'):
                    e2e_stats['block_number'] = extra_info.replace('block_', '')

        # Find emulator.preflight-execute
        if 'preflight-execute' in line or 'emulator.preflight_execute' in line:
            match = re.search(r'\[\s*([0-9.]+(?:ns|ms|µs|s))\s*\|', line)
            if match:
                e2e_stats['emulator_time'] = parse_time_to_seconds(match.group(1))

        # Find app_prove.inner
        if 'app_prove.inner' in line or 'app_prove_inner' in line:
            match = re.search(r'\[\s*([0-9.]+(?:ns|ms|µs|s))\s*\|', line)
            if match:
                app_prove_inner_time = parse_time_to_seconds(match.group(1))
                e2e_stats['app_prove_time'] = app_prove_inner_time

        # Find recursion.compress_to_root_proof
        if 'recursion.compress_to_root_proof' in line or 'compress_to_root_proof' in line:
            match = re.search(r'\[\s*([0-9.]+(?:ns|ms|µs|s))\s*\|', line)
            if match:
                e2e_stats['recursion_time'] = parse_time_to_seconds(match.group(1))

    if app_prove_inner_time == 0.0:
        print("Warning: Could not find app_prove.inner timing in the log")
        app_prove_inner_time = 1.0  # placeholder

    # Second pass: collect all relevant operations and chip breakdowns
    current_chip = None
    chip_indent_level = 0
    current_module = None
    module_indent_level = 0

    for i, line in enumerate(lines):
        operation, time_seconds, extra_info, indent = parse_trace_line(line)

        # Collect GPU operations within module blocks (must be before "if not operation" check)
        if current_module and '[ceno-gpu]' in line:
            # Parse the ceno-gpu operation
            gpu_pattern = r'\[ceno-gpu\]\s+([\w._-]+)\s*\[\s*([0-9.]+(?:ns|ms|µs|s))\s*\|'
            gpu_match = re.search(gpu_pattern, line)
            if gpu_match:
                gpu_op = gpu_match.group(1)
                gpu_time_str = gpu_match.group(2)
                gpu_time = parse_time_to_seconds(gpu_time_str)
                gpu_indent = get_indent_level(line)
                # Only collect if it's a child of the current module
                if gpu_indent > module_indent_level:
                    module_operations[current_module][gpu_op] += gpu_time

        if not operation:
            continue

        # Collect shard-level timings for overlap analysis
        if operation.endswith('generate_witness') and extra_info.startswith('shard_'):
            shard_id = int(extra_info.replace('shard_', ''))
            generate_witness_by_shard[shard_id] = time_seconds
        elif operation.endswith('create_proof_of_shard') and extra_info.startswith('shard_'):
            shard_id = int(extra_info.replace('shard_', ''))
            create_proof_by_shard[shard_id] = time_seconds

        # Track when we enter a module block (for GPU operations breakdown)
        # We update module context whenever we see these operations, even if nested
        if operation in ['commit_traces', 'prove_tower_relation_gpu', 'prove_main_constraints', 'pcs_opening']:
            current_module = operation
            module_indent_level = indent
        # Exit the module block when we see a sibling or parent operation (but not children)
        elif current_module and operation and indent <= module_indent_level:
            current_module = None

        # Track when we enter a create_chip_proof block
        if operation == 'create_chip_proof' and extra_info:
            current_chip = extra_info
            chip_indent_level = indent
            chip_proof_by_table[extra_info] += time_seconds
            continue  # Don't process this line further

        # Check if we've exited the current chip block
        if current_chip and indent <= chip_indent_level:
            current_chip = None

        # Collect operations for summary
        if operation in [
            'commit_traces',
            'prove_ec_sum_quark',
            'extract_witness_mles',
            'transport_structural_witness',
            'build_tower_witness_gpu',
            'prove_tower_relation_gpu',
            'prove_main_constraints',
            'pcs_opening',
        ]:
            total_by_operation[operation] += time_seconds

            # Also add to chip breakdown if we're inside a chip block (indent > chip level)
            if current_chip and indent > chip_indent_level:
                chip_operations[current_chip][operation] += time_seconds

    return app_prove_inner_time, total_by_operation, chip_proof_by_table, chip_operations, e2e_stats, generate_witness_by_shard, create_proof_by_shard, module_operations


def generate_summary_markdown(app_prove_inner_time: float,
                              total_by_operation: Dict[str, float],
                              chip_proof_by_table: Dict[str, float],
                              e2e_stats: Dict[str, float],
                              generate_witness_by_shard: Dict[int, float],
                              create_proof_by_shard: Dict[int, float]) -> str:
    """Generate summary Markdown table."""

    output = []

    # Header
    block_num = e2e_stats.get('block_number', 'N/A')
    e2e_time = e2e_stats.get('reth_block_time', 0.0)

    output.append(f"# Trace Profile Summary: block#{block_num}\n")
    output.append(f"**E2E Total Time: {e2e_time:.3f}s**\n")

    # E2E Overview Table
    output.append("## Table 1: E2E Overview\n")
    output.append("| Layer | Time (s) | % of E2E |")
    output.append("|-------|----------|----------|")

    emulator_time = e2e_stats.get('emulator_time', 0.0)
    app_prove_time = e2e_stats.get('app_prove_time', 0.0)
    recursion_time = e2e_stats.get('recursion_time', 0.0)

    if e2e_time > 0:
        output.append(f"| emulator | {emulator_time:.3f} | {(emulator_time/e2e_time*100):.2f}% |")
        output.append(f"| app_prove | {app_prove_time:.3f} | {(app_prove_time/e2e_time*100):.2f}% |")
        output.append(f"| recursion | {recursion_time:.3f} | {(recursion_time/e2e_time*100):.2f}% |")

        total_layers = emulator_time + app_prove_time + recursion_time
        output.append(f"| **TOTAL** | **{total_layers:.3f}** | **{(total_layers/e2e_time*100):.2f}%** |")

    output.append("")  # Empty line

    # Summary table
    output.append("## Table 2: App Prove Breakdown\n")
    output.append("| Operation | Time (s) | % of app_prove.inner |")
    output.append("|-----------|----------|---------------------|")

    operations_order = [
        'commit_traces',
        'prove_ec_sum_quark',
        'extract_witness_mles',
        'transport_structural_witness',
        'build_tower_witness_gpu',
        'prove_tower_relation_gpu',
        'prove_main_constraints',
        'pcs_opening',
    ]

    total_accounted = 0.0
    for op in operations_order:
        time = total_by_operation.get(op, 0.0)
        percentage = (time / app_prove_inner_time * 100) if app_prove_inner_time > 0 else 0.0
        output.append(f"| {op} | {time:.3f} | {percentage:.2f}% |")
        total_accounted += time

    # Add total row
    total_pct = (total_accounted / app_prove_inner_time * 100) if app_prove_inner_time > 0 else 0.0
    output.append(f"| **TOTAL** | **{total_accounted:.3f}** | **{total_pct:.2f}%** |")

    # CPU/GPU Overlap Table
    output.append("\n## Table 3: CPU/GPU Overlap\n")
    output.append("| CPU Shard | GPU Shard | CPU gen_witness (s) | GPU create_proof (s) | Overlap Gap (s) |")
    output.append("|-----------|-----------|---------------------|----------------------|-----------------|")

    # Sort shard IDs
    max_shard = max(list(generate_witness_by_shard.keys()) + list(create_proof_by_shard.keys())) if (generate_witness_by_shard or create_proof_by_shard) else 0

    total_gap = 0.0

    # First row: shard 0 (CPU only, GPU waiting for data)
    if 0 in generate_witness_by_shard:
        cpu_time = generate_witness_by_shard[0]
        gap = cpu_time  # shard 0's CPU time is all gap since GPU is waiting
        total_gap += gap
        output.append(f"| 0 | - | {cpu_time:.3f} | - | {gap:.3f} |")

    # Subsequent rows: shard N+1 CPU overlaps with shard N GPU
    for shard_id in range(1, max_shard + 1):
        if shard_id in generate_witness_by_shard:
            cpu_shard = shard_id
            gpu_shard = shard_id - 1
            cpu_time = generate_witness_by_shard.get(cpu_shard, 0.0)
            gpu_time = create_proof_by_shard.get(gpu_shard, 0.0)

            # Calculate gap: if CPU > GPU, there's a gap; otherwise 0
            gap = max(0, cpu_time - gpu_time)
            total_gap += gap

            gap_str = f"{gap:.3f}" if gap > 0 else "0"
            gpu_str = f"{gpu_time:.3f}" if gpu_time > 0 else "-"

            output.append(f"| {cpu_shard} | {gpu_shard} | {cpu_time:.3f} | {gpu_str} | {gap_str} |")

    # Total row
    output.append(f"| **TOTAL** | - | - | - | **{total_gap:.3f}** |")

    # Chip summary table
    output.append("\n## Table 4: Chip Proof Time\n")
    output.append("| Chip Name | Time (s) | % of app_prove.inner |")
    output.append("|-----------|----------|---------------------|")

    # Sort by time descending
    sorted_chips = sorted(chip_proof_by_table.items(), key=lambda x: x[1], reverse=True)

    total_chip_time = 0.0
    for table_name, time in sorted_chips:
        percentage = (time / app_prove_inner_time * 100) if app_prove_inner_time > 0 else 0.0
        output.append(f"| {table_name} | {time:.3f} | {percentage:.2f}% |")
        total_chip_time += time

    # Add total row
    total_chip_pct = (total_chip_time / app_prove_inner_time * 100) if app_prove_inner_time > 0 else 0.0
    output.append(f"| **TOTAL** | **{total_chip_time:.3f}** | **{total_chip_pct:.2f}%** |")

    return "\n".join(output)


def generate_breakdown_chip_markdown(app_prove_inner_time: float,
                                     chip_proof_by_table: Dict[str, float],
                                     chip_operations: Dict[str, Dict[str, float]],
                                     block_num: str) -> str:
    """Generate chip breakdown Markdown with top 10 chips."""

    output = []

    # Header
    output.append(f"# Top 10 Chip Breakdown: block#{block_num}\n")
    output.append(f"**Total app_prove.inner time: {app_prove_inner_time:.3f}s**\n")

    # Get top 10 chips by total time
    sorted_chips = sorted(chip_proof_by_table.items(), key=lambda x: x[1], reverse=True)[:10]

    operations_order = [
        'prove_ec_sum_quark',
        'extract_witness_mles',
        'transport_structural_witness',
        'build_tower_witness_gpu',
        'prove_tower_relation_gpu',
        'prove_main_constraints',
    ]

    for rank, (chip_name, total_time) in enumerate(sorted_chips, 1):
        output.append(f"## {rank}. {chip_name}\n")
        output.append(f"**Total time: {total_time:.3f}s ({(total_time/app_prove_inner_time*100):.2f}% of app_prove.inner)**\n")

        output.append(f"| Operation | Time (s) | % of chip total |")
        output.append("|-----------|----------|-----------------|")

        chip_ops = chip_operations.get(chip_name, {})
        accounted = 0.0

        for op in operations_order:
            op_time = chip_ops.get(op, 0.0)
            if op_time > 0:  # Only show operations that exist for this chip
                pct_of_chip = (op_time / total_time * 100) if total_time > 0 else 0.0
                output.append(f"| {op} | {op_time:.3f} | {pct_of_chip:.2f}% |")
                accounted += op_time

        # Add total row
        total_pct = (accounted / total_time * 100) if total_time > 0 else 0.0
        output.append(f"| **TOTAL** | **{accounted:.3f}** | **{total_pct:.2f}%** |")

        output.append("")  # Empty line between tables

    return "\n".join(output)


def generate_breakdown_module_markdown(app_prove_inner_time: float,
                                       total_by_operation: Dict[str, float],
                                       module_operations: Dict[str, Dict[str, float]],
                                       block_num: str) -> str:
    """Generate module breakdown Markdown with GPU operations for 4 main modules."""

    output = []

    # Header
    output.append(f"# GPU Module Breakdown: block#{block_num}\n")
    output.append(f"**Total app_prove.inner time: {app_prove_inner_time:.3f}s**\n")

    # Main modules to analyze
    modules = ['commit_traces', 'prove_tower_relation_gpu', 'prove_main_constraints', 'pcs_opening']

    # Table 1: Summary of 4 modules
    output.append("## Table 1: Modules Summary\n")
    output.append("| Module | Time (s) | % of app_prove.inner |")
    output.append("|--------|----------|---------------------|")

    total_module_time = 0.0
    for module in modules:
        module_time = total_by_operation.get(module, 0.0)
        if module_time > 0:
            percentage = (module_time / app_prove_inner_time * 100) if app_prove_inner_time > 0 else 0.0
            output.append(f"| {module} | {module_time:.3f} | {percentage:.2f}% |")
            total_module_time += module_time

    # Add total row
    total_pct = (total_module_time / app_prove_inner_time * 100) if app_prove_inner_time > 0 else 0.0
    output.append(f"| **TOTAL** | **{total_module_time:.3f}** | **{total_pct:.2f}%** |")
    output.append("")

    # Define GPU operations for each module
    module_gpu_ops = {
        'commit_traces': [
            'extract_poly_groups_from_rmms',
            'encode_poly_groups_to_codeword_matrices',
            'mmcs_matrices',
            'mmcs_polygroups',
            'mmcs_polygroups_one_by_one',
            'mmcs_rmms_one_by_one',
        ],
        'prove_tower_relation_gpu': [
            'prove_generic_sumcheck_gpu',
        ],
        'prove_main_constraints': [
            'prove_generic_sumcheck_gpu',
        ],
        'pcs_opening': [
            'batch_commit_phase',
            'batch_query_phase',
        ],
    }

    # Table 2-5: Breakdown for each module
    table_num = 2
    for module in modules:
        module_time = total_by_operation.get(module, 0.0)
        if module_time == 0:
            continue

        output.append(f"## Table {table_num}: {module}\n")
        output.append(f"**Total time: {module_time:.3f}s**\n")
        output.append("| GPU Operation | Time (s) | % of module total |")
        output.append("|---------------|----------|-------------------|")

        gpu_ops = module_operations.get(module, {})
        expected_ops = module_gpu_ops.get(module, [])

        total_gpu_time = 0.0
        for op in expected_ops:
            op_time = gpu_ops.get(op, 0.0)
            if op_time > 0:
                pct_of_module = (op_time / module_time * 100) if module_time > 0 else 0.0
                output.append(f"| {op} | {op_time:.3f} | {pct_of_module:.2f}% |")
                total_gpu_time += op_time

        # Add total row
        total_pct = (total_gpu_time / module_time * 100) if module_time > 0 else 0.0
        output.append(f"| **TOTAL** | **{total_gpu_time:.3f}** | **{total_pct:.2f}%** |")
        output.append("")

        table_num += 1

    return "\n".join(output)


def main():
    """Main entry point."""
    import sys

    if len(sys.argv) < 2:
        print("Usage: python analyze_trace.py <trace_log_file>")
        print("Example: python analyze_trace.py trace.log")
        sys.exit(1)

    trace_file = sys.argv[1]

    try:
        app_prove_inner_time, total_by_operation, chip_proof_by_table, chip_operations, e2e_stats, generate_witness_by_shard, create_proof_by_shard, module_operations = analyze_trace_log(trace_file)

        # Generate summary
        summary_md = generate_summary_markdown(app_prove_inner_time, total_by_operation, chip_proof_by_table, e2e_stats, generate_witness_by_shard, create_proof_by_shard)
        summary_file = trace_file.replace('.log', '_summary.md')
        with open(summary_file, 'w', encoding='utf-8') as f:
            f.write(summary_md)
        print(f"Summary saved to: {summary_file}")

        # Generate chip breakdown
        block_num = e2e_stats.get('block_number', 'N/A')
        breakdown_md = generate_breakdown_chip_markdown(app_prove_inner_time, chip_proof_by_table, chip_operations, block_num)
        breakdown_file = trace_file.replace('.log', '_breakdown_chip.md')
        with open(breakdown_file, 'w', encoding='utf-8') as f:
            f.write(breakdown_md)
        print(f"Chip breakdown saved to: {breakdown_file}")

        # Generate module breakdown
        module_md = generate_breakdown_module_markdown(app_prove_inner_time, total_by_operation, module_operations, block_num)
        module_file = trace_file.replace('.log', '_breakdown_module.md')
        with open(module_file, 'w', encoding='utf-8') as f:
            f.write(module_md)
        print(f"Module breakdown saved to: {module_file}")

    except FileNotFoundError:
        print(f"Error: File '{trace_file}' not found")
        sys.exit(1)
    except Exception as e:
        print(f"Error: {e}")
        import traceback
        traceback.print_exc()
        sys.exit(1)


if __name__ == "__main__":
    main()
