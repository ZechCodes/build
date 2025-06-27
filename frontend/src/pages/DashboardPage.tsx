import { useState, useEffect } from 'react';
import { Terminal } from '../components/Terminal/Terminal';
import { tokenService } from '../services/TokenService';

export function DashboardPage() {
  const [authToken, setAuthToken] = useState<string>('');
  const [tokenLoading, setTokenLoading] = useState(true);

  // Fetch authentication token on component mount
  useEffect(() => {
    const fetchToken = async () => {
      try {
        setTokenLoading(true);
        const token = await tokenService.getDemoToken();
        setAuthToken(token);
      } catch (error) {
        console.error('Failed to fetch auth token:', error);
        // Fallback to demo token
        setAuthToken('demo-token');
      } finally {
        setTokenLoading(false);
      }
    };

    fetchToken();
  }, []);
  return (
    <div className="min-h-screen bg-background text-foreground">
      <div className="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8 py-8">
        <div className="mb-8">
          <h1 className="text-3xl font-bold text-gray-900 dark:text-white">Dashboard</h1>
          <p className="mt-2 text-gray-600 dark:text-gray-400">
            Manage your development environments and projects
          </p>
        </div>

        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-6 mb-8">
          <div className="bg-white dark:bg-gray-800 overflow-hidden shadow rounded-lg">
            <div className="p-5">
              <div className="flex items-center">
                <div className="flex-shrink-0">
                  <div className="w-8 h-8 bg-blue-500 rounded-full flex items-center justify-center">
                    <span className="text-white text-sm font-medium">5</span>
                  </div>
                </div>
                <div className="ml-5 w-0 flex-1">
                  <dl>
                    <dt className="text-sm font-medium text-gray-500 dark:text-gray-400 truncate">
                      Active VMs
                    </dt>
                    <dd className="text-lg font-medium text-gray-900 dark:text-white">5</dd>
                  </dl>
                </div>
              </div>
            </div>
          </div>

          <div className="bg-white dark:bg-gray-800 overflow-hidden shadow rounded-lg">
            <div className="p-5">
              <div className="flex items-center">
                <div className="flex-shrink-0">
                  <div className="w-8 h-8 bg-green-500 rounded-full flex items-center justify-center">
                    <span className="text-white text-sm font-medium">12</span>
                  </div>
                </div>
                <div className="ml-5 w-0 flex-1">
                  <dl>
                    <dt className="text-sm font-medium text-gray-500 dark:text-gray-400 truncate">
                      Snapshots
                    </dt>
                    <dd className="text-lg font-medium text-gray-900 dark:text-white">12</dd>
                  </dl>
                </div>
              </div>
            </div>
          </div>

          <div className="bg-white dark:bg-gray-800 overflow-hidden shadow rounded-lg">
            <div className="p-5">
              <div className="flex items-center">
                <div className="flex-shrink-0">
                  <div className="w-8 h-8 bg-purple-500 rounded-full flex items-center justify-center">
                    <span className="text-white text-sm font-medium">3</span>
                  </div>
                </div>
                <div className="ml-5 w-0 flex-1">
                  <dl>
                    <dt className="text-sm font-medium text-gray-500 dark:text-gray-400 truncate">
                      Active Sessions
                    </dt>
                    <dd className="text-lg font-medium text-gray-900 dark:text-white">3</dd>
                  </dl>
                </div>
              </div>
            </div>
          </div>
        </div>

        <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
          <div className="bg-card shadow rounded-lg">
            <div className="px-4 py-5 sm:p-6">
              <h3 className="text-lg leading-6 font-medium text-card-foreground mb-4">
                Recent VMs
              </h3>
              <div className="space-y-3">
                {[1, 2, 3].map((vm) => (
                  <div key={vm} className="flex items-center justify-between p-3 bg-muted/50 rounded-lg">
                    <div className="flex items-center space-x-3">
                      <div className="w-3 h-3 bg-green-400 rounded-full"></div>
                      <div>
                        <p className="text-sm font-medium text-card-foreground">
                          Development VM {vm}
                        </p>
                        <p className="text-xs text-muted-foreground">
                          Ubuntu 22.04 • 2 vCPU • 4GB RAM
                        </p>
                      </div>
                    </div>
                    <div className="flex space-x-2">
                      <button className="text-primary hover:text-primary/80 text-sm font-medium">
                        Connect
                      </button>
                      <button className="text-muted-foreground hover:text-foreground text-sm font-medium">
                        Snapshot
                      </button>
                    </div>
                  </div>
                ))}
              </div>
            </div>
          </div>

          <div className="bg-card shadow rounded-lg">
            <div className="px-4 py-5 sm:p-6">
              <h3 className="text-lg leading-6 font-medium text-card-foreground mb-4">
                Terminal - Development VM 1
              </h3>
              {tokenLoading ? (
                <div className="flex items-center justify-center h-96 bg-muted/50 rounded-lg">
                  <div className="text-center">
                    <div className="animate-spin rounded-full h-8 w-8 border-b-2 border-primary mx-auto mb-2"></div>
                    <p className="text-sm text-muted-foreground">Loading authentication...</p>
                  </div>
                </div>
              ) : (
                <Terminal
                  vmId="vm-123"
                  height="400px"
                  apiUrl="ws://localhost:8000"
                  token={authToken}
                  theme="dark"
                  onSessionCreated={(sessionId) => console.log('Session created:', sessionId)}
                  onSessionEnded={() => console.log('Session ended')}
                />
              )}
            </div>
          </div>
        </div>
      </div>
    </div>
  )
}