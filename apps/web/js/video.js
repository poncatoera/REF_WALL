// Counts the new pictures a <video> has shown, so each is sent to the
// graphics card once and not again on every redraw.
const shown = new WeakMap();

export function watchFrames(video) {
  if (!video.requestVideoFrameCallback) return;
  shown.set(video, 0);
  const next = () => {
    shown.set(video, shown.get(video) + 1);
    video.requestVideoFrameCallback(next);
  };
  video.requestVideoFrameCallback(next);
}

// -1 where the browser can't tell.
export function framesShown(video) {
  return shown.get(video) ?? -1;
}

// Lets go of the decoder and its buffers; the element stays.
export function unloadVideo(video) {
  video.pause();
  video.removeAttribute("src");
  video.load();
}

// Takes it up again where it was. load() forgets the speed otherwise.
export function reloadVideo(video, url, time, rate) {
  video.defaultPlaybackRate = rate;
  video.src = url;
  video.currentTime = time; // becomes the start position once it has loaded
}
