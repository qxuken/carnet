<script lang="ts">
  // The camera, reading a QR code: what another device shows under
  // Settings → Sync → Add a device. The decoder (jsQR) loads only when
  // this opens.
  import { onDestroy, onMount } from 'svelte';
  import Icon from '../lib/Icon.svelte';

  let { found, close }: { found: (text: string) => void; close: () => void } = $props();
  let video: HTMLVideoElement | undefined = $state();
  let problem = $state<string | null>(null);
  let stream: MediaStream | null = null;
  let frame = 0;
  let done = false;

  onMount(async () => {
    try {
      stream = await navigator.mediaDevices.getUserMedia({ video: { facingMode: 'environment' }, audio: false });
    } catch (e) {
      const name = e instanceof DOMException ? e.name : '';
      problem =
        name === 'NotAllowedError'
          ? 'Carnet may not use the camera. Allow it in the browser’s settings for this site, or type the code.'
          : 'No camera could be opened here. Type the code instead.';
      return;
    }
    if (done || !video) return stop();
    video.srcObject = stream;
    await video.play().catch(() => {});
    const { default: jsQR } = await import('jsqr');
    const canvas = document.createElement('canvas');
    const ctx = canvas.getContext('2d', { willReadFrequently: true })!;
    const look = () => {
      if (done || !video) return;
      if (video.readyState >= 2 && video.videoWidth) {
        // A smaller copy reads as well, and faster.
        const k = Math.min(1, 640 / video.videoWidth);
        canvas.width = Math.round(video.videoWidth * k);
        canvas.height = Math.round(video.videoHeight * k);
        ctx.drawImage(video, 0, 0, canvas.width, canvas.height);
        const img = ctx.getImageData(0, 0, canvas.width, canvas.height);
        const code = jsQR(img.data, img.width, img.height, { inversionAttempts: 'attemptBoth' });
        if (code?.data) {
          done = true;
          stop();
          found(code.data);
          return;
        }
      }
      frame = requestAnimationFrame(look);
    };
    look();
  });

  function stop() {
    cancelAnimationFrame(frame);
    for (const t of stream?.getTracks() ?? []) t.stop();
    stream = null;
  }

  onDestroy(() => {
    done = true;
    stop();
  });
</script>

<div class="scan" role="dialog" aria-label="Scan a code">
  <!-- svelte-ignore a11y_media_has_caption -->
  <video bind:this={video} playsinline muted></video>
  <div class="frame" aria-hidden="true"></div>
  <p>{problem ?? 'Point the camera at the code on your other device.'}</p>
  <button onclick={close}><Icon name="cross" size={18} /> Cancel</button>
</div>

<style>
  .scan { position: fixed; inset: 0; z-index: 20; background: #000; display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 18px; padding: 24px; }
  video { position: absolute; inset: 0; width: 100%; height: 100%; object-fit: cover; }
  .frame { position: relative; width: min(64vw, 280px); aspect-ratio: 1; border-radius: 18px; box-shadow: 0 0 0 100vmax #0000008c; outline: 2px solid #ffffffd9; }
  p { position: relative; color: #fff; text-align: center; max-width: 320px; margin: 0; font-size: 15px; }
  button { position: relative; display: inline-flex; align-items: center; gap: 6px; padding: 10px 18px; border: 0; border-radius: 999px; background: #ffffff26; color: #fff; font-weight: 600; cursor: pointer; backdrop-filter: blur(10px); }
</style>
