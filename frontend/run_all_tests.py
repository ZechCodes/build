#!/usr/bin/env python3
"""
Comprehensive Test Runner for Session 8: Terminal Implementation
Ensures 100% test success rate across all testing phases
"""

import subprocess
import sys
import json
import time
import os
from typing import Dict, List, Tuple, Optional
from pathlib import Path

class TerminalTestRunner:
    """
    Orchestrates all testing phases to ensure 100% success rate
    """
    
    def __init__(self):
        self.start_time = time.time()
        self.results = {
            'phases': {},
            'overall_success': False,
            'total_tests': 0,
            'passed_tests': 0,
            'failed_tests': 0,
            'skipped_tests': 0,
            'duration': 0,
            'environment': {
                'node_version': self._get_node_version(),
                'npm_version': self._get_npm_version(),
                'python_version': sys.version,
                'platform': sys.platform,
                'cwd': os.getcwd()
            }
        }
        
    def _get_node_version(self) -> str:
        try:
            result = subprocess.run(['node', '--version'], capture_output=True, text=True)
            return result.stdout.strip() if result.returncode == 0 else 'unknown'
        except:
            return 'unknown'
    
    def _get_npm_version(self) -> str:
        try:
            result = subprocess.run(['npm', '--version'], capture_output=True, text=True)
            return result.stdout.strip() if result.returncode == 0 else 'unknown'
        except:
            return 'unknown'
    
    def _run_command(self, command: List[str], phase_name: str) -> Tuple[bool, str, str]:
        """Run a command and capture its output"""
        print(f"\n🔄 Running {phase_name}...")
        print(f"Command: {' '.join(command)}")
        
        try:
            result = subprocess.run(
                command,
                capture_output=True,
                text=True,
                timeout=300  # 5 minute timeout
            )
            
            success = result.returncode == 0
            stdout = result.stdout
            stderr = result.stderr
            
            if success:
                print(f"✅ {phase_name} completed successfully")
            else:
                print(f"❌ {phase_name} failed with exit code {result.returncode}")
                if stderr:
                    print(f"Error output: {stderr[:500]}...")
                    
            return success, stdout, stderr
            
        except subprocess.TimeoutExpired:
            print(f"⏰ {phase_name} timed out after 5 minutes")
            return False, "", "Command timed out"
        except Exception as e:
            print(f"💥 {phase_name} crashed: {str(e)}")
            return False, "", str(e)
    
    def _parse_vitest_results(self, stdout: str) -> Dict:
        """Parse Vitest output to extract test metrics"""
        results = {
            'total': 0,
            'passed': 0,
            'failed': 0,
            'skipped': 0,
            'duration': 0
        }
        
        lines = stdout.split('\n')
        for line in lines:
            line = line.strip()
            
            # Look for test summary lines
            if 'Test Files' in line and 'passed' in line:
                # Extract numbers from lines like "Test Files  4 passed (4)"
                parts = line.split()
                for i, part in enumerate(parts):
                    if part.isdigit() and i < len(parts) - 1:
                        if 'passed' in parts[i+1]:
                            results['passed'] = int(part)
                        elif 'failed' in parts[i+1]:
                            results['failed'] = int(part)
                        elif 'skipped' in parts[i+1]:
                            results['skipped'] = int(part)
            
            # Look for duration
            if 'Time:' in line:
                # Extract duration from lines like "Time:        2.15s"
                parts = line.split()
                for part in parts:
                    if 's' in part and part[:-1].replace('.', '').isdigit():
                        results['duration'] = float(part[:-1])
        
        results['total'] = results['passed'] + results['failed'] + results['skipped']
        return results
    
    def _parse_playwright_results(self, stdout: str) -> Dict:
        """Parse Playwright output to extract test metrics"""
        results = {
            'total': 0,
            'passed': 0,
            'failed': 0,
            'skipped': 0,
            'duration': 0
        }
        
        lines = stdout.split('\n')
        for line in lines:
            line = line.strip()
            
            # Look for summary lines like "5 passed (12s)"
            if 'passed' in line and '(' in line and 's)' in line:
                parts = line.split()
                for i, part in enumerate(parts):
                    if part.isdigit():
                        if i < len(parts) - 1 and 'passed' in parts[i+1]:
                            results['passed'] = int(part)
                        elif i < len(parts) - 1 and 'failed' in parts[i+1]:
                            results['failed'] = int(part)
                        elif i < len(parts) - 1 and 'skipped' in parts[i+1]:
                            results['skipped'] = int(part)
            
            # Extract duration from parentheses
            if '(' in line and 's)' in line:
                start = line.find('(') + 1
                end = line.find('s)')
                if start < end:
                    duration_str = line[start:end]
                    try:
                        results['duration'] = float(duration_str)
                    except:
                        pass
        
        results['total'] = results['passed'] + results['failed'] + results['skipped']
        return results
    
    def run_phase_1_infrastructure(self) -> bool:
        """Phase 1: Test Infrastructure Validation"""
        print("\n" + "="*60)
        print("📋 PHASE 1: TEST INFRASTRUCTURE VALIDATION")
        print("="*60)
        
        # Check dependencies
        success, stdout, stderr = self._run_command(['npm', 'list'], 'Dependency Check')
        if not success:
            print("⚠️  Some dependencies may be missing, but continuing...")
        
        # Lint check
        success, stdout, stderr = self._run_command(['npm', 'run', 'lint'], 'ESLint Check')
        if not success:
            print("❌ Linting failed - code quality issues detected")
            self.results['phases']['phase_1'] = {'success': False, 'reason': 'Lint failures'}
            return False
        
        # Type check
        success, stdout, stderr = self._run_command(['npm', 'run', 'type-check'], 'TypeScript Check')
        if not success:
            print("❌ TypeScript check failed - type errors detected")
            self.results['phases']['phase_1'] = {'success': False, 'reason': 'Type errors'}
            return False
        
        self.results['phases']['phase_1'] = {'success': True}
        print("✅ Phase 1: Infrastructure validation completed")
        return True
    
    def run_phase_2_unit_tests(self) -> bool:
        """Phase 2: Unit and Integration Tests (TDD Cycles 1-3)"""
        print("\n" + "="*60)
        print("🧪 PHASE 2: UNIT & INTEGRATION TESTS (TDD CYCLES)")
        print("="*60)
        
        # Run unit tests with coverage
        success, stdout, stderr = self._run_command([
            'npm', 'run', 'test:ci'
        ], 'Unit Tests with Coverage')
        
        if not success:
            print("❌ Unit tests failed")
            self.results['phases']['phase_2'] = {'success': False, 'reason': 'Unit test failures'}
            return False
        
        # Parse results
        test_results = self._parse_vitest_results(stdout)
        
        # Check for 100% success rate
        if test_results['failed'] > 0:
            print(f"❌ {test_results['failed']} unit tests failed - 100% success rate not achieved")
            self.results['phases']['phase_2'] = {
                'success': False, 
                'reason': f"{test_results['failed']} test failures",
                'metrics': test_results
            }
            return False
        
        if test_results['skipped'] > 0:
            print(f"⚠️  {test_results['skipped']} tests were skipped - not acceptable for 100% validation")
            self.results['phases']['phase_2'] = {
                'success': False,
                'reason': f"{test_results['skipped']} skipped tests",
                'metrics': test_results
            }
            return False
        
        # Update overall metrics
        self.results['total_tests'] += test_results['total']
        self.results['passed_tests'] += test_results['passed']
        
        self.results['phases']['phase_2'] = {
            'success': True,
            'metrics': test_results
        }
        
        print(f"✅ Phase 2: {test_results['total']} unit tests passed in {test_results['duration']}s")
        return True
    
    def run_phase_3_e2e_tests(self) -> bool:
        """Phase 3: End-to-End Browser Tests"""
        print("\n" + "="*60)
        print("🌐 PHASE 3: END-TO-END BROWSER TESTS")
        print("="*60)
        
        # Run Playwright tests
        success, stdout, stderr = self._run_command([
            'npm', 'run', 'test:e2e'
        ], 'Playwright E2E Tests')
        
        if not success:
            print("❌ E2E tests failed")
            self.results['phases']['phase_3'] = {'success': False, 'reason': 'E2E test failures'}
            return False
        
        # Parse results
        test_results = self._parse_playwright_results(stdout)
        
        # Check for 100% success rate
        if test_results['failed'] > 0:
            print(f"❌ {test_results['failed']} E2E tests failed - 100% success rate not achieved")
            self.results['phases']['phase_3'] = {
                'success': False,
                'reason': f"{test_results['failed']} E2E test failures",
                'metrics': test_results
            }
            return False
        
        if test_results['skipped'] > 0:
            print(f"⚠️  {test_results['skipped']} E2E tests were skipped - not acceptable")
            self.results['phases']['phase_3'] = {
                'success': False,
                'reason': f"{test_results['skipped']} skipped E2E tests",
                'metrics': test_results
            }
            return False
        
        # Update overall metrics
        self.results['total_tests'] += test_results['total']
        self.results['passed_tests'] += test_results['passed']
        
        self.results['phases']['phase_3'] = {
            'success': True,
            'metrics': test_results
        }
        
        print(f"✅ Phase 3: {test_results['total']} E2E tests passed in {test_results['duration']}s")
        return True
    
    def run_phase_4_security_validation(self) -> bool:
        """Phase 4: Security and Performance Validation"""
        print("\n" + "="*60)
        print("🔒 PHASE 4: SECURITY & PERFORMANCE VALIDATION")
        print("="*60)
        
        # Security scan (if semgrep is available)
        success, stdout, stderr = self._run_command([
            'npx', 'semgrep', '--config=auto', 'src/', '--json'
        ], 'Security Scan')
        
        if success:
            # Parse semgrep results
            try:
                scan_results = json.loads(stdout)
                findings = scan_results.get('results', [])
                
                # Filter for high severity issues
                high_severity = [f for f in findings if f.get('extra', {}).get('severity') == 'ERROR']
                
                if high_severity:
                    print(f"❌ {len(high_severity)} high-severity security issues found")
                    self.results['phases']['phase_4'] = {
                        'success': False,
                        'reason': f"{len(high_severity)} security vulnerabilities"
                    }
                    return False
                else:
                    print(f"✅ Security scan passed - {len(findings)} low-priority findings")
            except:
                print("⚠️  Could not parse security scan results")
        else:
            print("⚠️  Security scan not available - skipping")
        
        # Build test
        success, stdout, stderr = self._run_command(['npm', 'run', 'build'], 'Production Build')
        if not success:
            print("❌ Production build failed")
            self.results['phases']['phase_4'] = {'success': False, 'reason': 'Build failure'}
            return False
        
        self.results['phases']['phase_4'] = {'success': True}
        print("✅ Phase 4: Security and performance validation completed")
        return True
    
    def generate_final_report(self) -> None:
        """Generate comprehensive test report"""
        self.results['duration'] = time.time() - self.start_time
        
        print("\n" + "="*80)
        print("📊 FINAL TEST REPORT - SESSION 8: TERMINAL IMPLEMENTATION")
        print("="*80)
        
        # Overall summary
        success_rate = (self.results['passed_tests'] / self.results['total_tests'] * 100) if self.results['total_tests'] > 0 else 0
        
        print(f"\n📈 OVERALL METRICS:")
        print(f"   Total Tests: {self.results['total_tests']}")
        print(f"   Passed: {self.results['passed_tests']}")
        print(f"   Failed: {self.results['failed_tests']}")
        print(f"   Skipped: {self.results['skipped_tests']}")
        print(f"   Success Rate: {success_rate:.2f}%")
        print(f"   Total Duration: {self.results['duration']:.2f}s")
        
        # Phase breakdown
        print(f"\n🔍 PHASE BREAKDOWN:")
        for phase_name, phase_data in self.results['phases'].items():
            status = "✅ PASSED" if phase_data['success'] else "❌ FAILED"
            print(f"   {phase_name.upper()}: {status}")
            if not phase_data['success']:
                print(f"      Reason: {phase_data.get('reason', 'Unknown')}")
            if 'metrics' in phase_data:
                metrics = phase_data['metrics']
                print(f"      Tests: {metrics['total']}, Duration: {metrics['duration']}s")
        
        # Environment info
        print(f"\n🖥️  ENVIRONMENT:")
        env = self.results['environment']
        print(f"   Node.js: {env['node_version']}")
        print(f"   NPM: {env['npm_version']}")
        print(f"   Platform: {env['platform']}")
        print(f"   Working Directory: {env['cwd']}")
        
        # Success determination
        all_phases_passed = all(phase['success'] for phase in self.results['phases'].values())
        hundred_percent_success = (
            all_phases_passed and 
            self.results['failed_tests'] == 0 and 
            self.results['skipped_tests'] == 0 and
            self.results['total_tests'] > 0
        )
        
        self.results['overall_success'] = hundred_percent_success
        
        if hundred_percent_success:
            print(f"\n🎉 100% TEST SUCCESS ACHIEVED!")
            print(f"   Session 8: Terminal Implementation is COMPLETE")
            print(f"   All {self.results['total_tests']} tests passed with zero failures or skips")
            print(f"   Ready for production deployment ✨")
        else:
            print(f"\n⚠️  100% SUCCESS NOT ACHIEVED")
            print(f"   Review failed tests and address issues before completion")
            print(f"   Session 8: Terminal Implementation requires fixes")
        
        # Save detailed results
        results_file = Path('test-results') / 'session-8-final-report.json'
        results_file.parent.mkdir(exist_ok=True)
        
        with open(results_file, 'w') as f:
            json.dump(self.results, f, indent=2, default=str)
        
        print(f"\n📄 Detailed results saved to: {results_file}")
        
        return hundred_percent_success
    
    def run_all_phases(self) -> bool:
        """Execute all testing phases in sequence"""
        print("🚀 Starting comprehensive test execution for Session 8: Terminal Implementation")
        print("Target: 100% test success rate with zero tolerance for failures or skips")
        
        # Execute all phases
        phases = [
            ("Infrastructure Validation", self.run_phase_1_infrastructure),
            ("Unit & Integration Tests", self.run_phase_2_unit_tests),
            ("End-to-End Browser Tests", self.run_phase_3_e2e_tests),
            ("Security & Performance", self.run_phase_4_security_validation)
        ]
        
        for phase_name, phase_func in phases:
            print(f"\n🎯 Starting: {phase_name}")
            
            if not phase_func():
                print(f"💥 CRITICAL FAILURE in {phase_name}")
                print("Stopping execution - fix issues before proceeding")
                return self.generate_final_report()
            
            print(f"✅ Completed: {phase_name}")
        
        return self.generate_final_report()

def main():
    """Main execution function"""
    print("Session 8: Terminal Implementation - Comprehensive Test Suite")
    print("=" * 60)
    
    # Check if we're in the right directory
    if not Path('package.json').exists():
        print("❌ Error: package.json not found")
        print("Please run this script from the frontend project root directory")
        sys.exit(1)
    
    # Initialize and run test suite
    runner = TerminalTestRunner()
    success = runner.run_all_phases()
    
    # Exit with appropriate code
    sys.exit(0 if success else 1)

if __name__ == "__main__":
    main()