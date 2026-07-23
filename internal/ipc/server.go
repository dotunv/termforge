package ipc

import (
	"fmt"
	"log"
	"net"
	"sync"
)

type Server struct {
	listener net.Listener
	addr     string
	port     int
	handler  Handler
	mu       sync.Mutex
	clients  map[*Client]bool
}

type Handler interface {
	HandleRequest(client *Client, req *Request)
}

func NewServer(handler Handler) *Server {
	return &Server{
		handler: handler,
		clients: make(map[*Client]bool),
	}
}

func (s *Server) Start() error {
	var err error
	s.listener, err = net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		return fmt.Errorf("listen: %w", err)
	}

	addr := s.listener.Addr().(*net.TCPAddr)
	s.port = addr.Port
	s.addr = fmt.Sprintf("127.0.0.1:%d", addr.Port)

	log.Printf("[ipc] listening on %s", s.addr)

	go s.acceptLoop()
	return nil
}

func (s *Server) Port() int {
	return s.port
}

func (s *Server) Addr() string {
	return s.addr
}

func (s *Server) acceptLoop() {
	for {
		conn, err := s.listener.Accept()
		if err != nil {
			log.Printf("[ipc] accept error: %v", err)
			continue
		}

		client := NewClient(conn)
		s.mu.Lock()
		s.clients[client] = true
		s.mu.Unlock()

		log.Printf("[ipc] client connected")

		go s.handleClient(client)
	}
}

func (s *Server) handleClient(client *Client) {
	defer func() {
		s.mu.Lock()
		delete(s.clients, client)
		s.mu.Unlock()
		client.Close()
		log.Printf("[ipc] client disconnected")
	}()

	for {
		req, err := ReadMessage(client.conn)
		if err != nil {
			return
		}
		s.handler.HandleRequest(client, req)
	}
}

func (s *Server) Broadcast(evt *Event) {
	s.mu.Lock()
	defer s.mu.Unlock()

	for client := range s.clients {
		if err := WriteEvent(client.conn, evt); err != nil {
			log.Printf("[ipc] broadcast error: %v", err)
		}
	}
}

func (s *Server) Close() {
	if s.listener != nil {
		s.listener.Close()
	}
}

type Client struct {
	conn net.Conn
}

func NewClient(conn net.Conn) *Client {
	return &Client{conn: conn}
}

func (c *Client) Send(resp *Response) error {
	return WriteMessage(c.conn, resp)
}

func (c *Client) Close() {
	c.conn.Close()
}
