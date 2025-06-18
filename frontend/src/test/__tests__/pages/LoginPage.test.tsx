import { describe, it, expect } from 'vitest'
import { render, screen } from '../../utils/test-utils'
import { LoginPage } from '../../../pages/LoginPage'

describe('LoginPage', () => {
  it('renders the login form', () => {
    render(<LoginPage />)
    
    expect(screen.getByText('Sign in to Build Platform')).toBeInTheDocument()
    expect(screen.getByLabelText(/email address/i)).toBeInTheDocument()
    expect(screen.getByLabelText(/password/i)).toBeInTheDocument()
    expect(screen.getByRole('button', { name: /sign in/i })).toBeInTheDocument()
  })

  it('renders form inputs with correct attributes', () => {
    render(<LoginPage />)
    
    const emailInput = screen.getByLabelText(/email address/i)
    const passwordInput = screen.getByLabelText(/password/i)
    
    expect(emailInput).toHaveAttribute('type', 'email')
    expect(emailInput).toHaveAttribute('autoComplete', 'email')
    expect(passwordInput).toHaveAttribute('type', 'password')
    expect(passwordInput).toHaveAttribute('autoComplete', 'current-password')
  })

  it('has accessible form labels', () => {
    render(<LoginPage />)
    
    expect(screen.getByLabelText(/email address/i)).toBeRequired()
    expect(screen.getByLabelText(/password/i)).toBeRequired()
  })
})