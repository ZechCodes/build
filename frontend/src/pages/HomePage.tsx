import { Link } from 'react-router-dom'

export function HomePage() {
  return (
    <div className="min-h-screen flex flex-col items-center justify-center bg-gradient-to-br from-blue-50 to-indigo-100 dark:from-gray-900 dark:to-gray-800">
      <div className="max-w-4xl mx-auto text-center px-4">
        <h1 className="text-6xl font-bold bg-gradient-to-r from-blue-600 to-purple-600 bg-clip-text text-transparent mb-6">
          Build Platform
        </h1>
        <p className="text-xl text-gray-600 dark:text-gray-300 mb-8 max-w-2xl mx-auto">
          Isolated development environments with browser-based terminals. 
          Build, test, and deploy your applications in secure, scalable VMs.
        </p>
        <div className="flex gap-4 justify-center">
          <Link
            to="/login"
            className="bg-blue-600 hover:bg-blue-700 text-white px-8 py-3 rounded-lg font-semibold transition-colors"
          >
            Get Started
          </Link>
          <Link
            to="/dashboard"
            className="border border-gray-300 dark:border-gray-600 hover:bg-gray-100 dark:hover:bg-gray-700 px-8 py-3 rounded-lg font-semibold transition-colors"
          >
            Dashboard
          </Link>
        </div>
      </div>
      
      <div className="mt-16 grid grid-cols-1 md:grid-cols-3 gap-8 max-w-4xl mx-auto px-4">
        <div className="bg-white dark:bg-gray-800 p-6 rounded-lg shadow-md">
          <h3 className="text-lg font-semibold mb-3">Isolated Environments</h3>
          <p className="text-gray-600 dark:text-gray-300">
            Each project runs in its own Firecracker VM with complete isolation and security.
          </p>
        </div>
        <div className="bg-white dark:bg-gray-800 p-6 rounded-lg shadow-md">
          <h3 className="text-lg font-semibold mb-3">Browser Terminal</h3>
          <p className="text-gray-600 dark:text-gray-300">
            Full-featured terminal access directly in your browser with real-time collaboration.
          </p>
        </div>
        <div className="bg-white dark:bg-gray-800 p-6 rounded-lg shadow-md">
          <h3 className="text-lg font-semibold mb-3">Snapshot & Restore</h3>
          <p className="text-gray-600 dark:text-gray-300">
            Save your work state instantly and restore to any previous snapshot.
          </p>
        </div>
      </div>
    </div>
  )
}