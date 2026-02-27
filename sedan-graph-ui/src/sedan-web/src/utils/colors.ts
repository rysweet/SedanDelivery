export const LABEL_COLORS = [
  '#4FC3F7',
  '#81C784',
  '#FFB74D',
  '#E57373',
  '#CE93D8',
  '#4DD0E1',
  '#AED581',
  '#F06292',
  '#FFD54F',
  '#90A4AE',
  '#A1887F',
  '#4DB6AC',
] as const

export type LabelColor = (typeof LABEL_COLORS)[number]

/**
 * Returns a consistent color for a given label string using a hash function.
 * The same label always maps to the same color.
 */
export function getLabelColor(label: string): string {
  let hash = 0
  for (const char of label) {
    hash = (hash * 31 + char.charCodeAt(0)) % LABEL_COLORS.length
  }
  return LABEL_COLORS[Math.abs(hash)]
}

/**
 * Returns the color for a node based on its first label.
 * Falls back to a neutral gray if no labels are present.
 */
export function getNodeColor(labels: string[]): string {
  return labels.length > 0 ? getLabelColor(labels[0]) : '#90A4AE'
}

/**
 * Returns a dimmed version of a color for unselected/hidden nodes.
 */
export function getDimmedColor(color: string): string {
  return `${color}40`
}

/**
 * Builds a mapping of all unique labels in the dataset to their colors.
 */
export function buildLabelColorMap(labels: string[]): Map<string, string> {
  const map = new Map<string, string>()
  for (const label of labels) {
    map.set(label, getLabelColor(label))
  }
  return map
}
