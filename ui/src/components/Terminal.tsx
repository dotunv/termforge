import { useEffect, useRef, useCallback } from 'react'
import { Terminal } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import '@xterm/xterm/css/xterm.css'
import { writeSession, resizeSession } from '../lib/daemon'
import { useStore } from '../lib/store'

interface TerminalProps {
  sessionId: string
}

export default function TerminalView({ sessionId }: TerminalProps) {
  const containerRef = useRef<HTMLDivElement>(null)
  const termRef = useRef<Terminal | null>(null)
  const fitAddonRef = useRef<FitAddon | null>(null)

  useEffect(() => {
    if (!containerRef.current) return

    const term = new Terminal({
      cursorBlink: true,
      fontSize: 14,
      fontFamily: "'SF Mono', 'Fira Code', 'Cascadia Code', monospace",
      theme: {
        background: '#0a0a0f',
        foreground: '#e0e0e0',
        cursor: '#529bba',
        selectionBackground: '#264f78',
      },
      allowProposedApi: true,
    })

    const fitAddon = new FitAddon()
    term.loadAddon(fitAddon)
    term.open(containerRef.current)
    fitAddon.fit()

    termRef.current = term
    fitAddonRef.current = fitAddon

    // Handle user input
    term.onData((data) => {
      writeSession(sessionId, data)
    })

    // Handle resize
    const resizeObserver = new ResizeObserver(() => {
      fitAddon.fit()
      const dims = fitAddon.proposeDimensions()
      if (dims) {
        resizeSession(sessionId, dims.cols, dims.rows)
      }
    })
    resizeObserver.observe(containerRef.current)

    return () => {
      resizeObserver.disconnect()
      term.dispose()
      termRef.current = null
    }
  }, [sessionId])

  // Listen for output events
  useEffect(() => {
    const handleEvent = (msg: any) => {
      if (msg.type === 'session.output' && msg.payload?.session_id === sessionId) {
        termRef.current?.write(msg.payload.data)
      }
    }

    window.daemon.onEvent(handleEvent)
    return () => {}
  }, [sessionId])

  return (
    <div
      ref={containerRef}
      style={{
        width: '100%',
        height: '100%',
        backgroundColor: '#0a0a0f',
      }}
    />
  )
}
