const FILLER_WORDS = new Set([
  "um", "uh", "uhh", "umm", "er", "like", "you know", "basically",
  "actually", "literally", "so", "right", "okay", "well",
]);

interface WordTokenProps {
  word: string;
  isFiller: boolean;
  highlightFillers: boolean;
  onClick?: () => void;
}

export function WordToken({ word, isFiller, highlightFillers, onClick }: WordTokenProps) {
  const isHighlighted = highlightFillers && isFiller;

  return (
    <span
      className={`inline cursor-pointer rounded px-0.5 transition-colors ${
        isHighlighted
          ? "bg-yellow-200 line-through"
          : "hover:bg-secondary"
      }`}
      onClick={onClick}
    >
      {word}
    </span>
  );
}

export function isFillerWord(word: string): boolean {
  return FILLER_WORDS.has(word.toLowerCase().replace(/[.,!?;:'"()]/g, ""));
}
