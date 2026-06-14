package service

type UserService struct{}

func (s UserService) Load(id string) string { return id }

func MakeService() UserService { return UserService{} }
