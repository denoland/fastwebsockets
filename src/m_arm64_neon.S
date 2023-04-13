/*
    Copyright 2023 Divy Srivastava <dj.srivastava23@gmail.com>

    Licensed under the Apache License, Version 2.0 (the "License");
    you may not use this file except in compliance with the License.
    You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

    Unless required by applicable law or agreed to in writing, software
    distributed under the License is distributed on an "AS IS" BASIS,
    WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
    See the License for the specific language governing permissions and
    limitations under the License.
*/

#ifdef __clang__
# define TYPE(__proc)
#else
# define TYPE(__proc) .type __proc, @function
#endif

#ifdef __MACH__
# define PROC_NAME(__proc) _ ## __proc
#else
# define PROC_NAME(__proc) __proc
#endif

.global	PROC_NAME(unmask)
TYPE(unmask)
.p2align	2
PROC_NAME(unmask):
	.cfi_startproc

	cbz	x2, end_unmask

	mov	x8, #0
	cmp	x2, #8             
	b.lo	fallback

	sub	x9, x2, #1                      
	cmp	x9, #3                          
	b.hi	fallback

	cmp	x2, #32                         
	b.hs	unmask_32

	mov	x8, #0
	b	unmask_8

unmask_32:
	and	x8, x2, #0xffffffffffffffe0
	ldp	q0, q1, [x1]
	add	x9, x0, #16                     
	mov	x10, x8

unmask_32_loop:                                 
	ldp	q2, q3, [x9, #-16]
	eor	v2.16b, v2.16b, v0.16b
	eor	v3.16b, v3.16b, v1.16b
	stp	q2, q3, [x9, #-16]
	add	x9, x9, #32                     
	subs	x10, x10, #32                   
	b.ne	unmask_32_loop

	cmp	x8, x2
	b.eq	end_unmask

	tst	x2, #0x18
	b.eq	fallback

unmask_8:
	mov	x10, x8
	and	x8, x2, #0xfffffffffffffff8
	ldr	d0, [x1]
	add	x9, x0, x10
	sub	x10, x10, x8

unmask_8_loop:                               
	ldr	d1, [x9]
	eor	v1.8b, v1.8b, v0.8b
	str	d1, [x9], #8
	adds	x10, x10, #8                    
	b.ne	unmask_8_loop
	cmp	x8, x2
	b.eq	end_unmask

fallback:                                
	and	x9, x8, #0x3
	ldrb	w9, [x1, x9]
	ldrb	w10, [x0, x8]
	eor	w9, w10, w9
	strb	w9, [x0, x8]
	add	x8, x8, #1       
	cmp	x2, x8
	b.ne	fallback

end_unmask:
	ret

	.cfi_endproc
