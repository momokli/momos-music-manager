#!/usr/bin/env python3
"""Inspect the Discogs-EffNet ONNX model: input/output names, shapes, dtypes, metadata."""
import sys
import onnxruntime as ort

MODEL = sys.argv[1] if len(sys.argv) > 1 else "models/discogs-effnet-bsdynamic-1.onnx"

so = ort.SessionOptions()
so.log_severity_level = 3
sess = ort.InferenceSession(MODEL, sess_options=so, providers=["CPUExecutionProvider"])

print("=== ONNX Runtime ===")
print("ort version:", ort.__version__)
print("providers:", sess.get_providers())
print()
print("=== Inputs ===")
for i in sess.get_inputs():
    print(f"  name={i.name!r} shape={i.shape} type={i.type}")
print("=== Outputs ===")
for o in sess.get_outputs():
    print(f"  name={o.name!r} shape={o.shape} type={o.type}")
print()
md = sess.get_modelmeta()
print("=== Model metadata ===")
print("producer:", md.producer_name)
print("domain:", md.domain)
print("description:", md.description)
print("custom_metadata_map:", md.custom_metadata_map)
