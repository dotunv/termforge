import { useStore } from './store'

function getDaemon() {
  if (!window.daemon) throw new Error('Daemon IPC not available - preload may not have loaded')
  return window.daemon
}

export async function connectDaemon(projectDir: string): Promise<boolean> {
  const result = await getDaemon().connect(projectDir)
  if (result.error) {
    console.error('Failed to connect:', result.error)
    return false
  }
  useStore.getState().setConnected(true)
  useStore.getState().setProjectDir(projectDir)
  getDaemon().subscribe()
  return true
}

export async function request(method: string, payload?: any): Promise<any> {
  return getDaemon().request(method, payload)
}

export function onDaemonEvent(callback: (msg: any) => void) {
  getDaemon().onEvent(callback)
}

// Convenience wrappers
export async function createSession(name: string, cwd: string) {
  const result = await request('session.create', { name, cwd })
  if (result?.id) {
    const sessions = await request('session.list')
    useStore.getState().setSessions(sessions || [])
    useStore.getState().setActiveSession(result.id)
  }
  return result
}

export async function writeSession(sessionId: string, data: string) {
  return request('session.write', { session_id: sessionId, data })
}

export async function resizeSession(sessionId: string, width: number, height: number) {
  return request('session.resize', { session_id: sessionId, width, height })
}

export async function closeSession(sessionId: string) {
  const result = await request('session.close', { session_id: sessionId })
  const sessions = await request('session.list')
  useStore.getState().setSessions(sessions || [])
  return result
}

export async function loadSessions() {
  const sessions = await request('session.list')
  useStore.getState().setSessions(sessions || [])
}

export async function loadTasks() {
  const tasks = await request('task.list')
  useStore.getState().setTasks(tasks || [])
}

export async function createTask(title: string) {
  const result = await request('task.create', { title })
  await loadTasks()
  return result
}

export async function activateTask(taskId: string) {
  const result = await request('task.activate', { task_id: taskId })
  useStore.getState().setActiveTask(taskId)
  await loadTasks()
  return result
}

export async function completeTask(taskId: string) {
  const result = await request('task.complete', { task_id: taskId })
  await loadTasks()
  return result
}

export async function loadEvents() {
  const events = await request('event.list')
  useStore.getState().setEvents(events || [])
}

export async function loadKnowledge() {
  const nodes = await request('knowledge.list')
  useStore.getState().setKnowledge(nodes || [])
}

export async function createKnowledge(title: string, content: string, nodeType: string) {
  const result = await request('knowledge.create', { title, content, node_type: nodeType })
  await loadKnowledge()
  return result
}

export async function loadDocker() {
  const status = await request('docker.status')
  useStore.getState().setDockerContainers(status?.containers || [])
}
