export function stripMarkdownCodeBlocks(text: string): string {
  return text.replace(/```[\s\S]*?```/g, (block) => block.replace(/[^\n]/g, " "));
}
