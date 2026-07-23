import { create } from 'zustand'

export interface Session {
  id: string
  name: string
  cwd: string
}

export interface Task {
  id: string
  title: string
  status: string
}

export interface Event {
  id: string
  type: string
  task_id?: string
  session_id?: string
  payload: string
  created_at: string
}

export interface KnowledgeNode {
  id: string
  title: string
  content: string
  node_type: string
}

export interface DockerContainer {
  id: string
  name: string
  image: string
  state: string
  status: string
}

interface AppState {
  connected: boolean
  projectDir: string
  sessions: Session[]
  activeSessionId: string | null
  tasks: Task[]
  activeTaskId: string | null
  events: Event[]
  knowledge: KnowledgeNode[]
  dockerContainers: DockerContainer[]

  setConnected: (connected: boolean) => void
  setProjectDir: (dir: string) => void
  setSessions: (sessions: Session[]) => void
  setActiveSession: (id: string | null) => void
  setTasks: (tasks: Task[]) => void
  setActiveTask: (id: string | null) => void
  setEvents: (events: Event[]) => void
  setKnowledge: (nodes: KnowledgeNode[]) => void
  setDockerContainers: (containers: DockerContainer[]) => void
}

export const useStore = create<AppState>((set) => ({
  connected: false,
  projectDir: '',
  sessions: [],
  activeSessionId: null,
  tasks: [],
  activeTaskId: null,
  events: [],
  knowledge: [],
  dockerContainers: [],

  setConnected: (connected) => set({ connected }),
  setProjectDir: (dir) => set({ projectDir: dir }),
  setSessions: (sessions) => set({ sessions }),
  setActiveSession: (id) => set({ activeSessionId: id }),
  setTasks: (tasks) => set({ tasks }),
  setActiveTask: (id) => set({ activeTaskId: id }),
  setEvents: (events) => set({ events }),
  setKnowledge: (nodes) => set({ knowledge: nodes }),
  setDockerContainers: (containers) => set({ dockerContainers: containers }),
}))
