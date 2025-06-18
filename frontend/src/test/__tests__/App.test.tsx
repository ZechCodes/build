import { describe, it, expect } from 'vitest'
import { render, screen } from '../utils/test-utils'
import App from '../../App'

describe('App', () => {
  it('renders without crashing', () => {
    render(<App />)
    expect(screen.getByText('Build Platform')).toBeInTheDocument()
  })

  it('displays the homepage content', () => {
    render(<App />)
    
    expect(screen.getByText('Build Platform')).toBeInTheDocument()
    expect(screen.getByText(/Isolated development environments/)).toBeInTheDocument()
    expect(screen.getByRole('link', { name: 'Get Started' })).toBeInTheDocument()
    expect(screen.getByRole('link', { name: 'Dashboard' })).toBeInTheDocument()
  })

  it('has working navigation links', () => {
    render(<App />)
    
    const getStartedLink = screen.getByRole('link', { name: 'Get Started' })
    const dashboardLink = screen.getByRole('link', { name: 'Dashboard' })
    
    expect(getStartedLink).toHaveAttribute('href', '/login')
    expect(dashboardLink).toHaveAttribute('href', '/dashboard')
  })
})