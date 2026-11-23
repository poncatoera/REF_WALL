// Makes a big video smaller in the browser with WebCodecs: decode the
// frames, scale (and turn upright) on a canvas, encode as H.264.
// Rust reads the file's sample table and writes the finished MP4; this
// module only moves frames.

// Can this browser decode the source and encode H.264 at the target size?
// Returns the encoder codec string to use, or a reason it can't.
export async function shrinkSupport(codec, description, out) {
  description = description.slice();
  if (typeof VideoDecoder === "undefined" || typeof VideoEncoder === "undefined") {
    return { error: "this browser can't convert videos" };
  }
  const dec = await VideoDecoder.isConfigSupported(decoderConfig(codec, description, out)).catch(() => null);
  if (!dec || !dec.supported) {
    const kind = codec.startsWith("hvc1") || codec.startsWith("hev1") ? "HEVC (iPhone)" : codec.split(".")[0];
    return { error: `this browser can't read ${kind} videos` };
  }
  for (const c of encoderCodecs(out)) {
    const r = await VideoEncoder.isConfigSupported(encoderConfig(c, out)).catch(() => null);
    if (r && r.supported) return { codec: c };
  }
  return { error: "this browser can't write H.264 video at this size" };
}

// Converts the samples of `file`. `samples` is a Float64Array of
// [offset, size, timeUs, key] per frame in decode order. `out` has srcWidth,
// srcHeight (as stored), width, height (upright), rotation (0/90/180/270), kbps, keyframeUs, fps and
// codec (from shrinkSupport). Returns { description, frames: [{data, time, key}] }.
// One conversion at a time: each holds a decoder, an encoder and its output.
let queue = Promise.resolve();

export function shrinkVideo(file, codec, description, samples, out, onProgress) {
  // Copy now: the arrays are views into Wasm memory, which can move while we wait.
  description = description.slice();
  samples = samples.slice();
  const run = queue.then(() => convert(file, codec, description, samples, out, onProgress));
  queue = run.catch(() => {});
  return run;
}

async function convert(file, codec, description, samples, out, onProgress) {
  const frames = [];
  let outDescription = null;
  let failure = null;
  const fail = (e) => { failure = failure || e; };

  const encoder = new VideoEncoder({
    output(chunk, meta) {
      if (meta && meta.decoderConfig && meta.decoderConfig.description) {
        outDescription = new Uint8Array(copyBuffer(meta.decoderConfig.description));
      }
      const data = new Uint8Array(chunk.byteLength);
      chunk.copyTo(data);
      frames.push({ data, time: chunk.timestamp, key: chunk.type === "key" });
    },
    error: fail,
  });
  encoder.configure(encoderConfig(out.codec, out));

  const rot = out.rotation;
  const canvas = new OffscreenCanvas(out.width, out.height);
  const ctx = canvas.getContext("2d");
  // The frame's size before turning it upright.
  const [dw, dh] = rot === 90 || rot === 270 ? [out.height, out.width] : [out.width, out.height];
  let lastKey = -Infinity;
  let encoded = 0;

  const decoder = new VideoDecoder({
    output(frame) {
      try {
        // A decoder that has the size wrong would give a cropped video.
        const seen = frame.visibleRect;
        if (seen && (seen.width < out.srcWidth * 0.9 || seen.height < out.srcHeight * 0.9)) {
          throw new Error("this browser reads the video at the wrong size");
        }
        let input = frame;
        const same = rot === 0 && frame.displayWidth === out.width && frame.displayHeight === out.height;
        if (!same) {
          ctx.save();
          if (rot === 90) { ctx.translate(out.width, 0); ctx.rotate(Math.PI / 2); }
          else if (rot === 180) { ctx.translate(out.width, out.height); ctx.rotate(Math.PI); }
          else if (rot === 270) { ctx.translate(0, out.height); ctx.rotate(-Math.PI / 2); }
          ctx.drawImage(frame, 0, 0, dw, dh);
          ctx.restore();
          input = new VideoFrame(canvas, { timestamp: frame.timestamp, duration: frame.duration ?? undefined });
        }
        const key = frame.timestamp - lastKey >= out.keyframeUs;
        if (key) lastKey = frame.timestamp;
        encoder.encode(input, { keyFrame: key });
        if (input !== frame) input.close();
        encoded++;
      } catch (e) {
        fail(e);
      } finally {
        frame.close();
      }
    },
    error: fail,
  });
  decoder.configure(decoderConfig(codec, description, out));

  const count = samples.length / 4;
  let i = 0;
  while (i < count && !failure) {
    // Read a run of neighbouring frames in one slice (at most ~8 MB).
    const start = samples[i * 4];
    let end = start;
    let j = i;
    while (j < count && samples[j * 4] === end && end - start < 8e6) {
      end += samples[j * 4 + 1];
      j++;
    }
    if (j === i) { end = start + samples[i * 4 + 1]; j = i + 1; }
    const buf = new Uint8Array(await file.slice(start, end).arrayBuffer());
    for (let k = i; k < j && !failure; k++) {
      const off = samples[k * 4] - start;
      const size = samples[k * 4 + 1];
      while ((decoder.decodeQueueSize > 8 || encoder.encodeQueueSize > 8) && !failure) {
        await new Promise((r) => setTimeout(r, 2));
      }
      decoder.decode(new EncodedVideoChunk({
        type: samples[k * 4 + 3] ? "key" : "delta",
        timestamp: samples[k * 4 + 2],
        data: buf.subarray(off, off + size),
      }));
    }
    i = j;
    onProgress(Math.min(encoded, count) / count);
  }
  try {
    if (!failure) await decoder.flush();
    if (!failure) await encoder.flush();
  } catch (e) {
    fail(e);
  }
  if (decoder.state !== "closed") decoder.close();
  if (encoder.state !== "closed") encoder.close();
  if (failure) throw failure instanceof Error ? failure : new Error(String(failure));
  if (!outDescription || !frames.length) throw new Error("the converter gave no video");
  frames.sort((a, b) => a.time - b.time);
  return { description: outDescription, frames };
}

// "Making clip.mp4 smaller… 40%", under the page's other messages.
export function shrinkProgress(text) {
  let e = document.getElementById("wall-progress");
  if (!text) {
    if (e) e.remove();
    return;
  }
  if (!e) {
    e = document.createElement("div");
    e.id = "wall-progress";
    e.style.cssText =
      "position:fixed;left:50%;bottom:56px;transform:translateX(-50%);padding:8px 16px;border-radius:8px;font:13px system-ui,sans-serif;background:#1c1d22f2;color:#e8e9ee;border:1px solid #3a3c44;pointer-events:none;max-width:90vw";
    e.setAttribute("role", "status");
    document.body.append(e);
  }
  e.textContent = text;
}

function decoderConfig(codec, description, out) {
  // The size matters: without it Edge takes HEVC for 1280x720 and crops it.
  const c = { codec, codedWidth: out.srcWidth, codedHeight: out.srcHeight, hardwareAcceleration: "no-preference" };
  if (description && description.length) c.description = description;
  return c;
}

function encoderConfig(codec, out) {
  return {
    codec,
    width: out.width,
    height: out.height,
    bitrate: out.kbps * 1000,
    // Constant keeps the file near the size asked for; "variable" can overshoot a lot.
    bitrateMode: "constant",
    framerate: out.fps,
    avc: { format: "avc" },
    latencyMode: "quality",
    hardwareAcceleration: "no-preference",
  };
}

// H.264 levels: 4.0 up to 1080p30, 4.2 up to 1080p60, else 5.1. High,
// then Main, then Baseline profile.
function encoderCodecs(out) {
  const rate = out.width * out.height * out.fps;
  const level = rate <= 1920 * 1088 * 30.5 ? "28" : rate <= 1920 * 1088 * 60.5 ? "2A" : "33";
  return ["6400", "4D00", "4200"].map((p) => `avc1.${p}${level}`);
}

function copyBuffer(src) {
  if (src instanceof ArrayBuffer) return src.slice(0);
  return src.buffer.slice(src.byteOffset, src.byteOffset + src.byteLength);
}
