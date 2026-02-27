import { expect, test, vi } from 'vitest'
import { render, screen, act } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { Toast } from '../Toast'

test('renders error message', () => {
  const onDismiss = vi.fn()
  render(<Toast message="Test error" type="error" onDismiss={onDismiss} />)
  expect(screen.getByRole('alert')).toHaveTextContent('Test error')
})

test('renders success message', () => {
  const onDismiss = vi.fn()
  render(<Toast message="Operation succeeded" type="success" onDismiss={onDismiss} />)
  expect(screen.getByRole('alert')).toHaveTextContent('Operation succeeded')
})

test('renders info message', () => {
  const onDismiss = vi.fn()
  render(<Toast message="Info notice" type="info" onDismiss={onDismiss} />)
  expect(screen.getByRole('alert')).toHaveTextContent('Info notice')
})

test('calls onDismiss when dismiss button is clicked', async () => {
  const user = userEvent.setup()
  const onDismiss = vi.fn()
  render(<Toast message="msg" type="error" onDismiss={onDismiss} />)
  await user.click(screen.getByLabelText('Dismiss notification'))
  expect(onDismiss).toHaveBeenCalledTimes(1)
})

test('auto-dismisses after duration', async () => {
  vi.useFakeTimers()
  const onDismiss = vi.fn()
  render(<Toast message="msg" type="info" onDismiss={onDismiss} durationMs={1000} />)
  expect(onDismiss).not.toHaveBeenCalled()
  await act(async () => {
    vi.advanceTimersByTime(1001)
  })
  expect(onDismiss).toHaveBeenCalledTimes(1)
  vi.useRealTimers()
})

test('does not auto-dismiss before duration elapses', async () => {
  vi.useFakeTimers()
  const onDismiss = vi.fn()
  render(<Toast message="msg" type="info" onDismiss={onDismiss} durationMs={3000} />)
  await act(async () => {
    vi.advanceTimersByTime(2000)
  })
  expect(onDismiss).not.toHaveBeenCalled()
  vi.useRealTimers()
})

test('has assertive aria-live for screen reader announcement', () => {
  const onDismiss = vi.fn()
  render(<Toast message="error occurred" type="error" onDismiss={onDismiss} />)
  const alert = screen.getByRole('alert')
  expect(alert).toHaveAttribute('aria-live', 'assertive')
})
