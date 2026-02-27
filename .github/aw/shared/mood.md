# Sentiment Analysis Patterns

This document defines sentiment analysis patterns for GitHub issue analysis workflows.

## Sentiment Categories

### Positive Indicators
- "thanks", "great", "awesome", "excellent", "perfect"
- "solved", "fixed", "resolved", "working"
- "appreciate", "helpful", "love"
- Issue closed quickly (< 24 hours)
- Multiple positive reactions (👍, ❤️, 🎉)

### Negative Indicators
- "bug", "broken", "crash", "fail", "error"
- "frustrated", "annoying", "terrible", "worst"
- "doesn't work", "not working", "stopped working"
- Issue open for extended period (> 30 days)
- Multiple complaints or duplicate issues
- Escalation keywords: "urgent", "critical", "blocker"

### Neutral Indicators
- "question", "how to", "clarification"
- "feature request", "enhancement", "suggestion"
- Documentation-related issues
- Standard status updates

## Sentiment Scoring

**Positive**: +1 to +3 (based on intensity and context)
**Negative**: -1 to -3 (based on severity and impact)
**Neutral**: 0

## Context Modifiers

- **Response time**: Quick responses improve sentiment
- **Issue age**: Stale issues indicate negative sentiment
- **Engagement**: High participation can indicate importance
- **Resolution**: Closed issues with positive feedback boost sentiment

## Analysis Guidelines

1. Analyze issue title, body, and comments
2. Weight recent comments more heavily
3. Consider issue labels (bug, enhancement, question)
4. Account for external factors (breaking changes, releases)
5. Identify trending patterns across multiple issues
