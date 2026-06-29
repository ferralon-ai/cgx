package go_sample

import "sync"

func Spawn(wg *sync.WaitGroup) {
	go worker()
	defer wg.Done()
}

func worker() {}
