// 图片查看器 composable（点击放大浏览，左右切换 / Esc 关闭）。
import { ref } from "vue";

export function useImageViewer() {
  const imageViewer = ref<{ list: string[]; index: number } | null>(null);

  function openImageViewer(list: string[], index: number) {
    imageViewer.value = { list, index };
  }

  function ivPrev() {
    if (!imageViewer.value) return;
    const n = imageViewer.value.list.length;
    imageViewer.value.index = (imageViewer.value.index - 1 + n) % n;
  }

  function ivNext() {
    if (!imageViewer.value) return;
    const n = imageViewer.value.list.length;
    imageViewer.value.index = (imageViewer.value.index + 1) % n;
  }

  function ivClose() {
    imageViewer.value = null;
  }

  return { imageViewer, openImageViewer, ivPrev, ivNext, ivClose };
}
