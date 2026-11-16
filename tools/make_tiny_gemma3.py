# Tiny RANDOM-weight gemma3-architecture GGUF (text only, 0.4 MB) used by CI to exercise the REAL llama-server
# (launch flags, health, chunked SSE, timings, API key) without downloading a real model.  usage: make_tiny_gemma3.py <out.gguf>
import sys
import numpy as np, gguf
OUT = sys.argv[1] if len(sys.argv) > 1 else "tiny-gemma3.gguf"
w = gguf.GGUFWriter(OUT, "gemma3")
E, H, KV, HD, FF, L, CTX = 64, 4, 2, 16, 128, 2, 512
w.add_name("tiny-gemma3-random"); w.add_context_length(CTX); w.add_embedding_length(E); w.add_block_count(L)
w.add_feed_forward_length(FF); w.add_head_count(H); w.add_head_count_kv(KV); w.add_key_length(HD); w.add_value_length(HD)
w.add_layer_norm_rms_eps(1e-6); w.add_sliding_window(128); w.add_rope_freq_base(1000000.0); w.add_file_type(0)
w.add_uint32("gemma3.attention.sliding_window_pattern", 6) if False else None
toks = ["<pad>", "<eos>", "<bos>", "<unk>", "\u2581"] + [chr(c) for c in range(97, 123)] + ["\u2581" + chr(c) for c in range(97, 123)] + [f"<0x{i:02X}>" for i in range(256)]
types = [3, 3, 3, 2, 1] + [1] * 52 + [6] * 256
w.add_tokenizer_model("llama"); w.add_token_list(toks); w.add_token_scores([0.0] * len(toks)); w.add_token_types(types)
w.add_bos_token_id(2); w.add_eos_token_id(1); w.add_pad_token_id(0); w.add_unk_token_id(3); w.add_add_bos_token(True)
w.add_chat_template("{% for m in messages %}{{ m['content'] }}{% endfor %}")
r = np.random.default_rng(0)
def t(*s): return (r.standard_normal(s) * 0.05).astype(np.float32)
V = len(toks)
w.add_tensor("token_embd.weight", t(V, E)); w.add_tensor("output_norm.weight", np.ones(E, np.float32))
for i in range(L):
    p = f"blk.{i}."
    w.add_tensor(p + "attn_norm.weight", np.ones(E, np.float32)); w.add_tensor(p + "post_attention_norm.weight", np.ones(E, np.float32))
    w.add_tensor(p + "ffn_norm.weight", np.ones(E, np.float32)); w.add_tensor(p + "post_ffw_norm.weight", np.ones(E, np.float32))
    w.add_tensor(p + "attn_q.weight", t(H * HD, E)); w.add_tensor(p + "attn_k.weight", t(KV * HD, E)); w.add_tensor(p + "attn_v.weight", t(KV * HD, E))
    w.add_tensor(p + "attn_output.weight", t(E, H * HD)); w.add_tensor(p + "attn_q_norm.weight", np.ones(HD, np.float32)); w.add_tensor(p + "attn_k_norm.weight", np.ones(HD, np.float32))
    w.add_tensor(p + "ffn_gate.weight", t(FF, E)); w.add_tensor(p + "ffn_up.weight", t(FF, E)); w.add_tensor(p + "ffn_down.weight", t(E, FF))
w.write_header_to_file(); w.write_kv_data_to_file(); w.write_tensors_to_file(); w.close()
print("ok")
