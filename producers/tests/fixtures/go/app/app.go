package app

import "example.com/producer-go-fixture/service"

func RenderUser(id string) string {
	svc := service.MakeService()
	return svc.Load(id)
}
