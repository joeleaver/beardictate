import sys, numpy as np, wave, sherpa_onnx
m, outdir, src = sys.argv[1:4]
cfg = sherpa_onnx.OfflineTtsConfig(model=sherpa_onnx.OfflineTtsModelConfig(vits=sherpa_onnx.OfflineTtsVitsModelConfig(
    model=f"{m}/en_US-ryan-medium.onnx", tokens=f"{m}/tokens.txt", data_dir=f"{m}/espeak-ng-data"), num_threads=4))
tts = sherpa_onnx.OfflineTts(cfg)
for i, line in enumerate(open(src).read().splitlines()):
    a = tts.generate(line, sid=0, speed=1.0)
    x = np.concatenate([np.zeros(4000, np.float32), np.array(a.samples, np.float32), np.zeros(8000, np.float32)])
    w = wave.open(f"{outdir}/n{i}.wav", "wb"); w.setnchannels(1); w.setsampwidth(2); w.setframerate(a.sample_rate)
    w.writeframes((np.clip(x, -1, 1) * 32767).astype(np.int16).tobytes()); w.close()
