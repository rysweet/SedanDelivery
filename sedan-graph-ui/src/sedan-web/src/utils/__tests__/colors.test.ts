import { describe, it, expect } from 'vitest'
import {
  getLabelColor,
  getNodeColor,
  LABEL_COLORS,
  buildLabelColorMap,
} from '../colors'

describe('getLabelColor', () => {
  it('returns a valid hex color string', () => {
    const color = getLabelColor('Person')
    expect(color).toMatch(/^#[0-9A-Fa-f]{6}$/)
  })

  it('is deterministic - same label always returns same color', () => {
    const label = 'Employee'
    const first = getLabelColor(label)
    const second = getLabelColor(label)
    const third = getLabelColor(label)
    expect(first).toBe(second)
    expect(second).toBe(third)
  })

  it('returns a color from the LABEL_COLORS palette', () => {
    const labels = ['Person', 'Company', 'Location', 'Event', 'Product']
    for (const label of labels) {
      const color = getLabelColor(label)
      expect(LABEL_COLORS).toContain(color)
    }
  })

  it('handles empty string without throwing', () => {
    expect(() => getLabelColor('')).not.toThrow()
    const color = getLabelColor('')
    expect(LABEL_COLORS).toContain(color)
  })

  it('handles single character labels', () => {
    const color = getLabelColor('A')
    expect(LABEL_COLORS).toContain(color)
  })

  it('returns different colors for different labels (statistically)', () => {
    const labels = ['Alpha', 'Beta', 'Gamma', 'Delta', 'Epsilon', 'Zeta']
    const colors = labels.map(getLabelColor)
    // Not all colors should be the same (they could collide but very unlikely)
    const uniqueColors = new Set(colors)
    expect(uniqueColors.size).toBeGreaterThan(1)
  })

  it('handles labels with special characters', () => {
    expect(() => getLabelColor('Node_Type')).not.toThrow()
    expect(() => getLabelColor('Type123')).not.toThrow()
    expect(() => getLabelColor('Type With Spaces')).not.toThrow()
  })
})

describe('getNodeColor', () => {
  it('returns the color of the first label', () => {
    const labels = ['Person', 'Employee']
    const result = getNodeColor(labels)
    const expected = getLabelColor('Person')
    expect(result).toBe(expected)
  })

  it('returns fallback gray for empty labels array', () => {
    const result = getNodeColor([])
    expect(result).toBe('#90A4AE')
  })

  it('handles single label', () => {
    const result = getNodeColor(['Company'])
    expect(result).toBe(getLabelColor('Company'))
  })
})

describe('LABEL_COLORS', () => {
  it('has exactly 12 colors', () => {
    expect(LABEL_COLORS).toHaveLength(12)
  })

  it('all colors are valid hex strings', () => {
    for (const color of LABEL_COLORS) {
      expect(color).toMatch(/^#[0-9A-Fa-f]{6}$/)
    }
  })

  it('all colors are unique', () => {
    const unique = new Set(LABEL_COLORS)
    expect(unique.size).toBe(LABEL_COLORS.length)
  })

  it('contains the accent color', () => {
    expect(LABEL_COLORS).toContain('#4FC3F7')
  })
})

describe('buildLabelColorMap', () => {
  it('builds a map with correct entries', () => {
    const labels = ['Person', 'Company', 'Location']
    const map = buildLabelColorMap(labels)
    expect(map.size).toBe(3)
    expect(map.get('Person')).toBe(getLabelColor('Person'))
    expect(map.get('Company')).toBe(getLabelColor('Company'))
    expect(map.get('Location')).toBe(getLabelColor('Location'))
  })

  it('returns empty map for empty input', () => {
    const map = buildLabelColorMap([])
    expect(map.size).toBe(0)
  })

  it('handles duplicate labels', () => {
    const labels = ['Person', 'Person', 'Company']
    const map = buildLabelColorMap(labels)
    expect(map.size).toBe(2)
  })
})
