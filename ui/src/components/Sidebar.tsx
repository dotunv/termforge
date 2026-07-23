import { useState } from 'react'
import { useStore } from '../lib/store'
import { createSession, activateTask, completeTask, createTask } from '../lib/daemon'
import {
  Terminal,
  CheckSquare,
  Square,
  Plus,
  Circle,
  CircleDot,
  CheckCircle2,
  Activity,
  Database,
  FileText,
  Play,
  SquareIcon,
} from 'lucide-react'

type Panel = 'tasks' | 'sessions' | 'events' | 'knowledge' | 'docker'

export default function Sidebar() {
  const [activePanel, setActivePanel] = useState<Panel>('tasks')
  const {
    sessions,
    activeSessionId,
    tasks,
    events,
    knowledge,
    dockerContainers,
    setActiveSession,
  } = useStore()

  const panels: { id: Panel; icon: React.ReactNode; label: string }[] = [
    { id: 'tasks', icon: <CheckSquare size={18} />, label: 'Tasks' },
    { id: 'sessions', icon: <Terminal size={18} />, label: 'Sessions' },
    { id: 'events', icon: <Activity size={18} />, label: 'Events' },
    { id: 'knowledge', icon: <FileText size={18} />, label: 'Knowledge' },
    { id: 'docker', icon: <Database size={18} />, label: 'Docker' },
  ]

  return (
    <div style={{
      width: 260,
      height: '100%',
      borderRight: '1px solid #1e1e2e',
      display: 'flex',
      flexDirection: 'column',
      backgroundColor: '#0d0d14',
    }}>
      {/* Panel tabs */}
      <div style={{
        display: 'flex',
        borderBottom: '1px solid #1e1e2e',
        padding: '4px',
        gap: '2px',
      }}>
        {panels.map((p) => (
          <button
            key={p.id}
            onClick={() => setActivePanel(p.id)}
            title={p.label}
            style={{
              flex: 1,
              padding: '8px 4px',
              border: 'none',
              borderRadius: '4px',
              cursor: 'pointer',
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              backgroundColor: activePanel === p.id ? '#1a1a2e' : 'transparent',
              color: activePanel === p.id ? '#529bba' : '#666',
              transition: 'all 0.15s',
            }}
          >
            {p.icon}
          </button>
        ))}
      </div>

      {/* Panel content */}
      <div style={{ flex: 1, overflow: 'auto', padding: '8px' }}>
        {activePanel === 'tasks' && <TasksPanel />}
        {activePanel === 'sessions' && <SessionsPanel />}
        {activePanel === 'events' && <EventsPanel />}
        {activePanel === 'knowledge' && <KnowledgePanel />}
        {activePanel === 'docker' && <DockerPanel />}
      </div>
    </div>
  )
}

function TasksPanel() {
  const { tasks, activeTaskId } = useStore()
  const [newTitle, setNewTitle] = useState('')

  const handleCreate = async () => {
    if (!newTitle.trim()) return
    await createTask(newTitle.trim())
    setNewTitle('')
  }

  return (
    <div>
      <div style={{ display: 'flex', gap: 4, marginBottom: 8 }}>
        <input
          value={newTitle}
          onChange={(e) => setNewTitle(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && handleCreate()}
          placeholder="New task..."
          style={{
            flex: 1,
            padding: '6px 8px',
            backgroundColor: '#12121c',
            border: '1px solid #1e1e2e',
            borderRadius: '4px',
            color: '#e0e0e0',
            fontSize: 12,
            outline: 'none',
          }}
        />
        <button
          onClick={handleCreate}
          style={{
            padding: '6px 8px',
            backgroundColor: '#1a1a2e',
            border: '1px solid #2e2e4e',
            borderRadius: '4px',
            color: '#529bba',
            cursor: 'pointer',
            display: 'flex',
            alignItems: 'center',
          }}
        >
          <Plus size={14} />
        </button>
      </div>
      {tasks.map((task) => (
        <div
          key={task.id}
          style={{
            padding: '6px 8px',
            marginBottom: 4,
            borderRadius: '4px',
            backgroundColor: task.id === activeTaskId ? '#1a1a2e' : 'transparent',
            cursor: 'pointer',
            display: 'flex',
            alignItems: 'center',
            gap: 8,
            fontSize: 12,
          }}
          onClick={() => activateTask(task.id)}
        >
          {task.status === 'completed' ? (
            <CheckCircle2 size={14} color="#4ade80" />
          ) : task.status === 'active' ? (
            <CircleDot size={14} color="#529bba" />
          ) : (
            <Circle size={14} color="#666" />
          )}
          <span style={{
            textDecoration: task.status === 'completed' ? 'line-through' : 'none',
            color: task.status === 'completed' ? '#666' : '#e0e0e0',
          }}>
            {task.title}
          </span>
        </div>
      ))}
    </div>
  )
}

function SessionsPanel() {
  const { sessions, activeSessionId } = useStore()

  const handleNew = async () => {
    await createSession('Terminal', '')
  }

  return (
    <div>
      <button
        onClick={handleNew}
        style={{
          width: '100%',
          padding: '6px 8px',
          marginBottom: 8,
          backgroundColor: '#1a1a2e',
          border: '1px solid #2e2e4e',
          borderRadius: '4px',
          color: '#529bba',
          cursor: 'pointer',
          fontSize: 12,
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          gap: 4,
        }}
      >
        <Plus size={14} /> New Session
      </button>
      {sessions.map((sess) => (
        <div
          key={sess.id}
          onClick={() => setActiveSession(sess.id)}
          style={{
            padding: '6px 8px',
            marginBottom: 4,
            borderRadius: '4px',
            backgroundColor: sess.id === activeSessionId ? '#1a1a2e' : 'transparent',
            cursor: 'pointer',
            display: 'flex',
            alignItems: 'center',
            gap: 8,
            fontSize: 12,
          }}
        >
          <Terminal size={14} color={sess.id === activeSessionId ? '#529bba' : '#666'} />
          <span>{sess.name}</span>
        </div>
      ))}
    </div>
  )
}

function EventsPanel() {
  const { events } = useStore()

  return (
    <div>
      {events.length === 0 && (
        <div style={{ color: '#666', fontSize: 12, textAlign: 'center', padding: 16 }}>
          No events yet
        </div>
      )}
      {events.slice().reverse().map((evt) => (
        <div
          key={evt.id}
          style={{
            padding: '6px 8px',
            marginBottom: 4,
            borderRadius: '4px',
            backgroundColor: '#12121c',
            fontSize: 11,
          }}
        >
          <div style={{ color: '#529bba', marginBottom: 2 }}>{evt.type}</div>
          <div style={{ color: '#888' }}>{evt.created_at}</div>
        </div>
      ))}
    </div>
  )
}

function KnowledgePanel() {
  const { knowledge } = useStore()
  const [showCreate, setShowCreate] = useState(false)
  const [title, setTitle] = useState('')
  const [content, setContent] = useState('')

  return (
    <div>
      <button
        onClick={() => setShowCreate(!showCreate)}
        style={{
          width: '100%',
          padding: '6px 8px',
          marginBottom: 8,
          backgroundColor: '#1a1a2e',
          border: '1px solid #2e2e4e',
          borderRadius: '4px',
          color: '#529bba',
          cursor: 'pointer',
          fontSize: 12,
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          gap: 4,
        }}
      >
        <Plus size={14} /> Add Note
      </button>
      {showCreate && (
        <div style={{ marginBottom: 8, padding: 8, backgroundColor: '#12121c', borderRadius: 4 }}>
          <input
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            placeholder="Title"
            style={{
              width: '100%',
              padding: '4px 8px',
              marginBottom: 4,
              backgroundColor: '#0a0a0f',
              border: '1px solid #1e1e2e',
              borderRadius: '4px',
              color: '#e0e0e0',
              fontSize: 12,
              outline: 'none',
            }}
          />
          <textarea
            value={content}
            onChange={(e) => setContent(e.target.value)}
            placeholder="Content..."
            rows={3}
            style={{
              width: '100%',
              padding: '4px 8px',
              backgroundColor: '#0a0a0f',
              border: '1px solid #1e1e2e',
              borderRadius: '4px',
              color: '#e0e0e0',
              fontSize: 12,
              outline: 'none',
              resize: 'vertical',
            }}
          />
          <button
            onClick={async () => {
              if (title.trim() && content.trim()) {
                const { createKnowledge } = await import('../lib/daemon')
                await createKnowledge(title.trim(), content.trim(), 'note')
                setTitle('')
                setContent('')
                setShowCreate(false)
              }
            }}
            style={{
              marginTop: 4,
              padding: '4px 12px',
              backgroundColor: '#529bba',
              border: 'none',
              borderRadius: '4px',
              color: '#0a0a0f',
              cursor: 'pointer',
              fontSize: 12,
            }}
          >
            Save
          </button>
        </div>
      )}
      {knowledge.map((node) => (
        <div
          key={node.id}
          style={{
            padding: '6px 8px',
            marginBottom: 4,
            borderRadius: '4px',
            backgroundColor: '#12121c',
            fontSize: 12,
          }}
        >
          <div style={{ color: '#529bba', marginBottom: 2 }}>{node.title}</div>
          <div style={{ color: '#888', whiteSpace: 'pre-wrap' }}>{node.content.slice(0, 100)}</div>
        </div>
      ))}
    </div>
  )
}

function DockerPanel() {
  const { dockerContainers } = useStore()

  return (
    <div>
      {dockerContainers.length === 0 && (
        <div style={{ color: '#666', fontSize: 12, textAlign: 'center', padding: 16 }}>
          No containers
        </div>
      )}
      {dockerContainers.map((c) => (
        <div
          key={c.id}
          style={{
            padding: '6px 8px',
            marginBottom: 4,
            borderRadius: '4px',
            backgroundColor: '#12121c',
            fontSize: 12,
          }}
        >
          <div style={{ display: 'flex', alignItems: 'center', gap: 6, marginBottom: 2 }}>
            {c.state === 'running' ? (
              <Play size={12} color="#4ade80" />
            ) : (
              <SquareIcon size={12} color="#666" />
            )}
            <span style={{ color: '#e0e0e0' }}>{c.name}</span>
          </div>
          <div style={{ color: '#888', fontSize: 11 }}>{c.image}</div>
        </div>
      ))}
    </div>
  )
}
