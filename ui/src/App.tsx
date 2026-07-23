import { useState, useEffect } from 'react'
import { useStore } from './lib/store'
import { connectDaemon, loadSessions, loadTasks, loadEvents, loadKnowledge, loadDocker, onDaemonEvent } from './lib/daemon'
import Sidebar from './components/Sidebar'
import TerminalView from './components/Terminal'

export default function App() {
  const { connected, activeSessionId, sessions, setActiveSession } = useStore()
  const [projectDir, setProjectDir] = useState('')
  const [connecting, setConnecting] = useState(false)
  const [error, setError] = useState('')

  const handleConnect = async () => {
    if (!projectDir.trim()) return
    setConnecting(true)
    setError('')
    const ok = await connectDaemon(projectDir.trim())
    if (ok) {
      await loadSessions()
      await loadTasks()
      await loadEvents()
      await loadKnowledge()
      await loadDocker()
    } else {
      setError('Failed to connect to daemon')
    }
    setConnecting(false)
  }

  // Listen for events and refresh data
  useEffect(() => {
    try {
      onDaemonEvent((msg) => {
        if (msg.type?.startsWith('session.')) loadSessions()
        if (msg.type?.startsWith('task.')) loadTasks()
        if (msg.type?.startsWith('file.')) loadEvents()
      })
    } catch (e) {
      // Daemon not available yet
    }
  }, [])

  if (!connected) {
    return (
      <div style={{
        height: '100vh',
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        backgroundColor: '#0a0a0f',
      }}>
        <div style={{
          padding: 40,
          backgroundColor: '#0d0d14',
          borderRadius: 8,
          border: '1px solid #1e1e2e',
          textAlign: 'center',
          minWidth: 400,
        }}>
          <h1 style={{
            fontSize: 24,
            fontWeight: 600,
            color: '#529bba',
            marginBottom: 8,
          }}>
            TermForge
          </h1>
          <p style={{ color: '#666', marginBottom: 24, fontSize: 13 }}>
            Persistent development workspace
          </p>
          <div style={{ display: 'flex', gap: 8 }}>
            <input
              value={projectDir}
              onChange={(e) => setProjectDir(e.target.value)}
              onKeyDown={(e) => e.key === 'Enter' && handleConnect()}
              placeholder="Project directory..."
              disabled={connecting}
              style={{
                flex: 1,
                padding: '10px 12px',
                backgroundColor: '#12121c',
                border: '1px solid #1e1e2e',
                borderRadius: '6px',
                color: '#e0e0e0',
                fontSize: 13,
                outline: 'none',
              }}
            />
            <button
              onClick={handleConnect}
              disabled={connecting || !projectDir.trim()}
              style={{
                padding: '10px 20px',
                backgroundColor: '#529bba',
                border: 'none',
                borderRadius: '6px',
                color: '#0a0a0f',
                cursor: 'pointer',
                fontSize: 13,
                fontWeight: 600,
              }}
            >
              {connecting ? 'Connecting...' : 'Connect'}
            </button>
          </div>
          {error && (
            <p style={{ color: '#ef4444', marginTop: 12, fontSize: 12 }}>{error}</p>
          )}
          <p style={{ color: '#444', marginTop: 16, fontSize: 11 }}>
            Run: forge &lt;project-dir&gt;
          </p>
        </div>
      </div>
    )
  }

  return (
    <div style={{ height: '100vh', display: 'flex', backgroundColor: '#0a0a0f' }}>
      <Sidebar />
      <div style={{ flex: 1, display: 'flex', flexDirection: 'column' }}>
        {/* Top bar */}
        <div style={{
          height: 36,
          borderBottom: '1px solid #1e1e2e',
          display: 'flex',
          alignItems: 'center',
          padding: '0 12px',
          gap: 8,
          backgroundColor: '#0d0d14',
        }}>
          <span style={{ color: '#529bba', fontSize: 12, fontWeight: 600 }}>TermForge</span>
          <span style={{ color: '#444', fontSize: 11 }}>|</span>
          <span style={{ color: '#666', fontSize: 11 }}>{useStore.getState().projectDir}</span>
        </div>

        {/* Terminal area */}
        <div style={{ flex: 1, overflow: 'hidden' }}>
          {activeSessionId ? (
            <TerminalView key={activeSessionId} sessionId={activeSessionId} />
          ) : (
            <div style={{
              height: '100%',
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
            }}>
              <div style={{ textAlign: 'center' }}>
                <p style={{ color: '#666', fontSize: 14, marginBottom: 8 }}>
                  No active session
                </p>
                <p style={{ color: '#444', fontSize: 12 }}>
                  Click "New Session" in the sidebar to start
                </p>
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  )
}
