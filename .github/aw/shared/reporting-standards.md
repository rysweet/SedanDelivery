# Reporting Standards for gh-aw Workflows

This document defines common reporting standards for all analytical workflows.

## Discussion Post Format

All analytical reports should follow this structure:

### 1. Executive Summary
- **Purpose**: One-sentence description of the report
- **Key Findings**: 3-5 bullet points of most important insights
- **Recommendations**: Actionable next steps

### 2. Detailed Analysis
- **Data Period**: Clear time range
- **Methodology**: How data was collected and analyzed
- **Visualizations**: Charts with proper labels and legends
- **Progressive Disclosure**: Use collapsible sections for details

### 3. Actionable Insights
- **Priorities**: What needs attention first
- **Trends**: Week-over-week or month-over-month changes
- **Risks**: Potential issues identified
- **Opportunities**: Areas for improvement

## Visualization Standards

### Chart Requirements
- **Resolution**: 300 DPI minimum for clarity
- **Dimensions**: 12 inches wide x 7 inches tall (landscape)
- **Fonts**: Minimum 10pt for labels, 12pt for titles
- **Colors**: Accessible color palette (colorblind-friendly)
- **Labels**: All axes clearly labeled with units

### Chart Types
- **Trends**: Line charts for time series
- **Comparisons**: Bar charts for categorical data
- **Distributions**: Histograms or box plots
- **Proportions**: Pie charts (limit to 5 categories max)

## Progressive Disclosure

Use collapsible sections for detailed information:

```markdown
<details>
<summary>📊 Detailed Metrics</summary>

[Detailed content here]

</details>
```

## Tone and Style

- **Professional**: Clear, factual language
- **Actionable**: Focus on what can be done
- **Data-Driven**: Support claims with metrics
- **Concise**: Respect reader's time
- **Transparent**: Acknowledge limitations and assumptions

## Formatting

- Use emoji sparingly for visual hierarchy
- Bold important numbers and findings
- Use tables for structured data
- Include timestamps and data sources
- Link to relevant issues, PRs, or documentation
