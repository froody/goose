#!/usr/bin/env python3
import sys

def run_simulation(num_files=3, avg_file_size_tokens=1000, system_prompt_tokens=5000):
    print("="*60)
    print(f"BENCHMARKING TOKEN SAVINGS: MULTI-FILE EDIT TASK ({num_files} files)")
    print(f"  - Base System Prompt: {system_prompt_tokens} tokens")
    print(f"  - Average File Size:  {avg_file_size_tokens} tokens")
    print("="*60)

    # ----------------------------------------------------
    # SCENARIO A: Traditional Multi-Turn Approach
    # ----------------------------------------------------
    a_turns = []
    current_context = system_prompt_tokens
    
    # 1. User prompt
    a_turns.append(("User Prompt", 0, current_context))
    
    # 2. Tree call
    a_turns.append(("Tool Call: tree", 10, current_context))
    current_context += 10 + 200 # response has 200 tokens of file list
    
    # 3. Search call (find files)
    a_turns.append(("Tool Call: search (findstr)", 15, current_context))
    current_context += 15 + 50 # response has 50 tokens
    
    for i in range(num_files):
        # Read file
        a_turns.append((f"Tool Call: read file {i+1}", 15, current_context))
        current_context += 15 + avg_file_size_tokens
        
        # Edit file
        a_turns.append((f"Tool Call: edit file {i+1}", 50, current_context))
        current_context += 50 + 50 # response has 50 tokens of success msg
        
    a_total_prompt_tokens = sum(turn[2] for turn in a_turns)
    a_total_turns = len(a_turns)

    # ----------------------------------------------------
    # SCENARIO B: Consolidated Batching Approach
    # ----------------------------------------------------
    b_turns = []
    current_context = system_prompt_tokens
    
    # 1. User prompt
    b_turns.append(("User Prompt", 0, current_context))
    
    # 2. Combined Search & Read
    b_turns.append(("Tool Call: search (exact match + read)", 40, current_context))
    current_context += 40 + (avg_file_size_tokens * num_files)
    
    # 3. Batch Edit
    b_turns.append(("Tool Call: edit_multiple", 150, current_context))
    current_context += 150 + 100 # response has 100 tokens of batch success
    
    b_total_prompt_tokens = sum(turn[2] for turn in b_turns)
    b_total_turns = len(b_turns)

    # ----------------------------------------------------
    # PRINT RESULTS
    # ----------------------------------------------------
    savings = (1.0 - (b_total_prompt_tokens / a_total_prompt_tokens)) * 100
    latency_reduction = (1.0 - (b_total_turns / a_total_turns)) * 100

    print(f"{'Metric':<25} | {'Scenario A (Legacy)':<22} | {'Scenario B (Batch)':<22}")
    print("-"*75)
    print(f"{'Total API Roundtrips':<25} | {a_total_turns:<22} | {b_total_turns:<22}")
    print(f"{'Total Prompt Tokens':<25} | {a_total_prompt_tokens:<22,} | {b_total_prompt_tokens:<22,}")
    print(f"{'Peak Context Window':<25} | {a_turns[-1][2]:<22,} | {b_turns[-1][2]:<22,}")
    print("-"*75)
    print(f"💰 Prompt Token Savings: {savings:.1f}%")
    print(f"⚡ Latency / Roundtrip Reduction: {latency_reduction:.1f}%")
    print("="*60)

    # Visual Bar Chart in terminal
    def render_bar(label, value, max_val, char="█"):
        bar_len = int((value / max_val) * 30)
        return f"{label:<25} | {char * bar_len:<30} ({value:,})"

    max_tokens = max(a_total_prompt_tokens, b_total_prompt_tokens)
    print("\nToken Usage Comparison Chart:")
    print(render_bar("Scenario A (Legacy)", a_total_prompt_tokens, max_tokens))
    print(render_bar("Scenario B (Batch)", b_total_prompt_tokens, max_tokens))
    print("="*60 + "\n")

if __name__ == '__main__':
    run_simulation(num_files=3)
    run_simulation(num_files=5)
