import { describe, it, expect } from 'vitest'
import { render, screen } from '../../utils/test-utils'
import { HomePage } from '../../../pages/HomePage'

describe('HomePage', () => {
  it('renders the main heading', () => {
    render(<HomePage />)
    expect(screen.getByText('Build Platform')).toBeInTheDocument()
  })

  it('renders the description', () => {
    render(<HomePage />)
    expect(screen.getByText(/Isolated development environments with browser-based terminals/)).toBeInTheDocument()
  })

  it('renders feature cards', () => {
    render(<HomePage />)
    
    expect(screen.getByText('Isolated Environments')).toBeInTheDocument()
    expect(screen.getByText('Browser Terminal')).toBeInTheDocument()
    expect(screen.getByText('Snapshot & Restore')).toBeInTheDocument()
  })

  it('renders call-to-action buttons', () => {
    render(<HomePage />)
    
    const getStartedButton = screen.getByRole('link', { name: 'Get Started' })
    const dashboardButton = screen.getByRole('link', { name: 'Dashboard' })
    
    expect(getStartedButton).toBeInTheDocument()
    expect(dashboardButton).toBeInTheDocument()
  })
})