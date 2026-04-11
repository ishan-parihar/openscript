import { useState, useCallback, DragEvent } from "react";
import { useProjectStore } from "../store/project";

const VIDEO_EXTENSIONS = ["mp4", "mov", "avi", "mkv", "webm", "m4v"];

interface UseFileDropOptions {
  onDrop?: (path: string) => void;
}

export function useFileDrop({ onDrop }: UseFileDropOptions = {}) {
  const [isDragging, setIsDragging] = useState(false);
  const { createProject } = useProjectStore();

  const handleDragEnter = useCallback((e: DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    setIsDragging(true);
  }, []);

  const handleDragLeave = useCallback((e: DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    setIsDragging(false);
  }, []);

  const handleDragOver = useCallback((e: DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
  }, []);

  const handleDrop = useCallback(
    async (e: DragEvent) => {
      e.preventDefault();
      e.stopPropagation();
      setIsDragging(false);

      const files = e.dataTransfer.files;
      if (files.length > 0) {
        const file = files[0];
        const ext = file.name.split(".").pop()?.toLowerCase();
        if (ext && VIDEO_EXTENSIONS.includes(ext)) {
          try {
            await createProject(file.name);
            onDrop?.(file.name);
          } catch (err) {
            console.error("Failed to create project from dropped file:", err);
          }
        }
      }
    },
    [createProject, onDrop]
  );

  return {
    isDragging,
    handleDragEnter,
    handleDragLeave,
    handleDragOver,
    handleDrop,
  };
}
