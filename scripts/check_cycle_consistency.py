#!/usr/bin/env python3
import json
import sys
import argparse

def get_cycles(metrics_path):
    try:
        with open(metrics_path) as f:
            data = json.load(f)
        for entry in data.get("gauge", []):
            if entry.get("metric") == "cycles":
                return int(float(entry.get("value")))
    except Exception as e:
        print(f"Error reading {metrics_path}: {e}")
    return None

def main():
    parser = argparse.ArgumentParser(description="Check cycle consistency between two metrics files.")
    parser.add_argument("base_metrics", help="Path to base metrics.json")
    parser.add_argument("target_metrics", help="Path to target metrics.json")
    parser.add_argument("--tolerance", type=float, default=0.0, help="Allowed relative difference (default 0.0 for exact match)")
    args = parser.parse_args()

    base_cycles = get_cycles(args.base_metrics)
    target_cycles = get_cycles(args.target_metrics)

    if base_cycles is None:
        print(f"Error: 'cycles' metric not found in {args.base_metrics}")
        sys.exit(1)
    if target_cycles is None:
        print(f"Error: 'cycles' metric not found in {args.target_metrics}")
        sys.exit(1)

    print(f"Base cycles: {base_cycles}")
    print(f"Target cycles: {target_cycles}")

    if base_cycles == 0:
        rel_diff = 0 if target_cycles == 0 else float('inf')
    else:
        rel_diff = abs(target_cycles - base_cycles) / base_cycles

    if rel_diff > args.tolerance:
        print(f"Cycle mismatch! Difference: {abs(target_cycles - base_cycles)} ({rel_diff:.2%})")
        sys.exit(1)
    else:
        print("Cycle consistency check passed.")
        sys.exit(0)

if __name__ == "__main__":
    main()
