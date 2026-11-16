package sample

import (
	"fmt"
	util "example.com/acme/app/internal/util"
)

type Base struct{}

type Server struct {
	Base
	name string
}

type Runner interface {
	Run() error
}

func (s *Server) Run() error {
	fmt.Println(util.Helper(s.name))
	return nil
}

func NewServer() *Server {
	return &Server{}
}

func main() {
	NewServer().Run()
}
